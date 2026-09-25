//! `warpctrl sync`: drives Warp Sync from outside Warp.
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use local_control::protocol::{
    ActionKind, ControlError, ErrorCode, SyncConfirmation, SyncPathParams, SyncPendingParams,
    SyncResult, SyncStatusParams, SyncUploadSummary,
};
use uuid::Uuid;

use crate::agent::OutputFormat;
use crate::local_control::commands::send_action;
use crate::local_control::output::{write_json, write_json_line};
use crate::local_control::{
    EXIT_NEEDS_CONFIRMATION, EXIT_SUCCESS, SyncCommand, SyncPathArgs, SyncPendingArgs, TargetArgs,
};

/// A Warp Sync operation is several remote commands that only run while the shell is idle, so it
/// takes far longer than an ordinary local-control request.
const SYNC_CLIENT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How many paths of a list are printed before the rest is summarized.
const MAX_LISTED_PATHS: usize = 10;

const BYTES_PER_KIB: u64 = 1024;
const BYTES_PER_MIB: u64 = 1024 * 1024;

pub(super) fn run_sync_command(
    command: SyncCommand,
    output_format: OutputFormat,
) -> Result<u8, ControlError> {
    let (target, action, params) = match command {
        SyncCommand::Status(args) => (
            args.target,
            ActionKind::SyncStatus,
            serde_json::to_value(SyncStatusParams {
                path: args.path.as_deref().map(absolute_path).transpose()?,
            }),
        ),
        SyncCommand::Download(args) => path_request(args, ActionKind::SyncDownload)?,
        SyncCommand::Upload(args) => path_request(args, ActionKind::SyncUploadPrepare)?,
        SyncCommand::Compare(args) => path_request(args, ActionKind::SyncCompare)?,
        SyncCommand::Confirm(args) => pending_request(args, ActionKind::SyncConfirm),
        SyncCommand::Cancel(args) => pending_request(args, ActionKind::SyncCancel),
    };
    let params = params.map_err(|err| {
        ControlError::with_details(
            ErrorCode::InvalidParams,
            "failed to serialize the sync parameters",
            err.to_string(),
        )
    })?;
    let data = send_action(&target, action, params, SYNC_CLIENT_TIMEOUT)?;

    match output_format {
        OutputFormat::Json => write_json(&data)?,
        OutputFormat::Ndjson => write_json_line(&data)?,
        OutputFormat::Pretty | OutputFormat::Text => println!("{}", render_sync_data(&data)),
    }
    Ok(exit_code(&data))
}

fn path_request(
    args: SyncPathArgs,
    action: ActionKind,
) -> Result<(TargetArgs, ActionKind, serde_json::Result<serde_json::Value>), ControlError> {
    let params = serde_json::to_value(SyncPathParams {
        path: absolute_path(&args.path)?,
    });
    Ok((args.target, action, params))
}

fn pending_request(
    args: SyncPendingArgs,
    action: ActionKind,
) -> (TargetArgs, ActionKind, serde_json::Result<serde_json::Value>) {
    let target = TargetArgs {
        instance: args.instance,
        pid: args.pid,
        ..TargetArgs::default()
    };
    let params = serde_json::to_value(SyncPendingParams {
        pending_id: args.pending_id,
    });
    (target, action, params)
}

/// Makes `path` absolute against the working directory, so that `warpctrl sync upload etc/nginx`
/// works from inside a mirror. The app still validates the result.
pub(super) fn absolute_path(path: &str) -> Result<String, ControlError> {
    std::path::absolute(Path::new(path))
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|err| {
            ControlError::with_details(
                ErrorCode::InvalidParams,
                "failed to make the path absolute",
                err.to_string(),
            )
        })
}

/// A confirmation request is not a failure, but scripts must be able to tell that nothing has
/// happened yet.
pub(super) fn exit_code(data: &serde_json::Value) -> u8 {
    match serde_json::from_value::<SyncResult>(data.clone()) {
        Ok(SyncResult::NeedsConfirmation { .. }) => EXIT_NEEDS_CONFIRMATION,
        Ok(_) | Err(_) => EXIT_SUCCESS,
    }
}

pub(super) fn render_sync_data(data: &serde_json::Value) -> String {
    match serde_json::from_value::<SyncResult>(data.clone()) {
        Ok(result) => render_sync_result(&result),
        Err(_) => serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string()),
    }
}

pub(super) fn render_sync_result(result: &SyncResult) -> String {
    match result {
        SyncResult::Status { mirror_root, path } => {
            let mut text = format!("Mirror folder: {mirror_root}");
            let Some(path) = path else {
                return text;
            };
            let _ = write!(text, "\nHost folder: {}", path.host_key);
            if let Some(remote_path) = &path.remote_path {
                let _ = write!(text, "\nRemote path: {remote_path}");
            }
            if path.sessions.is_empty() {
                text.push_str("\nNo open Warp session is connected to this host.");
            } else {
                text.push_str("\nSessions:");
                for session in &path.sessions {
                    let _ = write!(
                        text,
                        "\n  {}@{}: session {} (window {}, tab {}){}",
                        session.user,
                        session.hostname,
                        session.session_id,
                        session.window_id,
                        session.tab_index + 1,
                        if session.is_active { ", active" } else { "" }
                    );
                }
            }
            text
        }
        SyncResult::Downloaded {
            local_path,
            files,
            dirs,
            bytes,
            remote_user,
            skipped,
            baseline_warning,
        } => {
            let mut text = format!(
                "Downloaded {} and {} ({}) as {remote_user} to {local_path}",
                count(*files, "file"),
                count(*dirs, "folder"),
                format_size(*bytes),
            );
            for entry in skipped.iter().take(MAX_LISTED_PATHS) {
                let _ = write!(text, "\n  skipped {} ({})", entry.path, entry.reason);
            }
            append_remainder(&mut text, skipped.len());
            if let Some(warning) = baseline_warning {
                let _ = write!(text, "\n{warning}");
            }
            text
        }
        SyncResult::NeedsConfirmation {
            pending_id,
            confirmation,
        } => {
            let mut text = match confirmation {
                SyncConfirmation::OverwriteLocalChanges { files } => {
                    let mut text = format!(
                        "Downloading would overwrite local changes to {}:",
                        count(files.len() as u64, "file")
                    );
                    list_paths(&mut text, "  ", files);
                    text
                }
                SyncConfirmation::Upload { summary } => render_upload_summary(summary),
            };
            let _ = write!(
                text,
                "\nNothing has been changed yet. Run `warpctrl sync confirm {pending_id}` to go \
                 ahead, or `warpctrl sync cancel {pending_id}` to drop it."
            );
            text
        }
        SyncResult::Uploaded {
            files,
            dirs,
            bytes,
            remote_user,
            backup_path,
            baseline_warning,
        } => {
            let mut text = format!(
                "Uploaded {} and {} ({}) as {remote_user}",
                count(*files, "file"),
                count(*dirs, "folder"),
                format_size(*bytes),
            );
            if let Some(backup_path) = backup_path {
                let _ = write!(text, "\nPrevious version saved to {backup_path}");
            }
            if let Some(warning) = baseline_warning {
                let _ = write!(text, "\n{warning}");
            }
            text
        }
        SyncResult::Compared {
            differences,
            identical_files,
            diff_path,
            remote_user,
            ..
        } => {
            let mut text = format!(
                "{} between the server (as {remote_user}) and the mirror; {} identical",
                count(differences.len() as u64, "difference"),
                count(*identical_files, "file"),
            );
            for difference in differences.iter().take(MAX_LISTED_PATHS) {
                let _ = write!(
                    text,
                    "\n  {}: {}",
                    change_label(difference.change),
                    difference.remote_path
                );
            }
            append_remainder(&mut text, differences.len());
            let _ = write!(text, "\nDiff saved at {diff_path}");
            text
        }
        SyncResult::Unchanged { identical_files } => format!(
            "No differences: the mirror matches the server ({})",
            count(*identical_files, "file")
        ),
        SyncResult::Cancelled => "Cancelled. Nothing was changed.".to_owned(),
    }
}

fn render_upload_summary(summary: &SyncUploadSummary) -> String {
    let mut text = format!(
        "Upload {} to {}@{}{}: {} and {} ({})",
        summary.remote_path,
        summary.remote_user,
        summary.hostname,
        summary
            .server_id_tail
            .as_deref()
            .map(|tail| format!(" (machine id ending {tail})"))
            .unwrap_or_default(),
        count(summary.files, "file"),
        count(summary.dirs, "folder"),
        format_size(summary.bytes),
    );
    if !summary.new_files.is_empty() {
        text.push_str("\nNew files:");
        list_paths(&mut text, "  ", &summary.new_files);
    }
    if !summary.missing_locally.is_empty() {
        text.push_str("\nMissing from the mirror (they will not be deleted on the server):");
        list_paths(&mut text, "  ", &summary.missing_locally);
    }
    match &summary.remote_conflicts {
        None => text.push_str(
            "\nWARNING: the server could not be checked for changes made since the last sync.",
        ),
        Some(conflicts) => {
            for (heading, paths) in [
                (
                    "WARNING: changed on the server since the last sync (the upload overwrites \
                     them):",
                    &conflicts.changed,
                ),
                (
                    "WARNING: synced before but gone from the server:",
                    &conflicts.missing,
                ),
                (
                    "WARNING: new here but already on the server (the upload overwrites them):",
                    &conflicts.already_exist,
                ),
            ] {
                if !paths.is_empty() {
                    let _ = write!(text, "\n{heading}");
                    list_paths(&mut text, "  ", paths);
                }
            }
        }
    }
    if summary.ownership_may_be_incomplete {
        text.push_str("\nNote: the server's tar is not GNU tar, so ownership may not be restored completely.");
    }
    text
}

fn list_paths(text: &mut String, indent: &str, paths: &[String]) {
    for path in paths.iter().take(MAX_LISTED_PATHS) {
        let _ = write!(text, "\n{indent}{path}");
    }
    append_remainder(text, paths.len());
}

fn append_remainder(text: &mut String, total: usize) {
    if let Some(more) = total.checked_sub(MAX_LISTED_PATHS).filter(|more| *more > 0) {
        let _ = write!(text, "\n  ... and {more} more");
    }
}

fn change_label(change: local_control::protocol::SyncChange) -> &'static str {
    use local_control::protocol::SyncChange;
    match change {
        SyncChange::ChangedLocally => "changed locally",
        SyncChange::ChangedOnServer => "changed on the server",
        SyncChange::ChangedOnBoth => "changed on both sides",
        SyncChange::Differs => "differs",
        SyncChange::NewOnServer => "new on the server",
        SyncChange::DeletedLocally => "deleted locally",
        SyncChange::NewLocally => "new locally",
        SyncChange::DeletedOnServer => "deleted on the server",
    }
}

fn count(count: u64, noun: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{suffix}")
}

fn format_size(bytes: u64) -> String {
    if bytes >= BYTES_PER_MIB {
        format!("{:.1} MiB", bytes as f64 / BYTES_PER_MIB as f64)
    } else if bytes >= BYTES_PER_KIB {
        format!("{:.1} KiB", bytes as f64 / BYTES_PER_KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Parses a pending id typed on the command line.
pub(super) fn parse_pending_id(value: &str) -> Result<Uuid, String> {
    Uuid::parse_str(value).map_err(|_| "expected the id printed by `warpctrl sync upload`".to_owned())
}
