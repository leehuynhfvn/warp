//! Shapes what Warp Sync produced into the local-control wire types.

use std::path::Path;

use ::local_control::protocol::{
    SyncChange, SyncConfirmation, SyncDifference, SyncRemoteConflicts, SyncResult,
    SyncSkippedEntry, SyncUploadSummary,
};
use ::local_control::{ControlError, ErrorCode};

use crate::warp_sync::remote_check::RemoteCheck;
use crate::warp_sync::{
    ConfirmationKind, FileChange, FileDifference, SyncReply, UploadSummary, WarpSyncError,
    printable,
};

pub(super) fn sync_result(reply: SyncReply) -> SyncResult {
    match reply {
        SyncReply::Downloaded {
            local_path,
            files,
            dirs,
            bytes,
            remote_user,
            skipped,
            baseline_warning,
        } => SyncResult::Downloaded {
            local_path: display(&local_path),
            files: files as u64,
            dirs: dirs as u64,
            bytes,
            remote_user,
            skipped: skipped
                .into_iter()
                .map(|entry| SyncSkippedEntry {
                    path: entry.path,
                    reason: entry.reason,
                })
                .collect(),
            baseline_warning,
        },
        SyncReply::NeedsConfirmation { pending_id, kind } => SyncResult::NeedsConfirmation {
            pending_id,
            confirmation: confirmation(kind),
        },
        SyncReply::Uploaded {
            files,
            dirs,
            bytes,
            remote_user,
            backup_path,
            baseline_warning,
        } => SyncResult::Uploaded {
            files: files as u64,
            dirs: dirs as u64,
            bytes,
            remote_user,
            backup_path,
            baseline_warning,
        },
        SyncReply::Compared {
            differences,
            identical_files,
            diff_path,
            host_dir,
            server_copy_dir,
            remote_user,
        } => SyncResult::Compared {
            differences: differences.into_iter().map(difference).collect(),
            identical_files: identical_files as u64,
            diff_path: display(&diff_path),
            host_dir: display(&host_dir),
            server_copy_dir: display(&server_copy_dir),
            remote_user,
        },
        SyncReply::Unchanged { identical_files } => SyncResult::Unchanged {
            identical_files: identical_files as u64,
        },
    }
}

fn confirmation(kind: ConfirmationKind) -> SyncConfirmation {
    match kind {
        ConfirmationKind::OverwriteLocalChanges { files } => {
            SyncConfirmation::OverwriteLocalChanges { files }
        }
        ConfirmationKind::Upload(summary) => SyncConfirmation::Upload {
            summary: Box::new(upload_summary(*summary)),
        },
    }
}

fn upload_summary(summary: UploadSummary) -> SyncUploadSummary {
    SyncUploadSummary {
        remote_user: summary.remote_user,
        hostname: summary.hostname,
        remote_path: summary.remote_path,
        files: summary.files as u64,
        dirs: summary.dirs as u64,
        bytes: summary.content_bytes,
        new_files: summary.new_files,
        missing_locally: summary.missing_locally,
        remote_conflicts: match summary.remote_check {
            RemoteCheck::Unavailable => None,
            RemoteCheck::Checked(conflicts) => Some(SyncRemoteConflicts {
                changed: conflicts.changed,
                missing: conflicts.missing,
                already_exist: conflicts.already_exist,
            }),
        },
        ownership_may_be_incomplete: summary.ownership_may_be_incomplete,
        server_id_tail: summary.server_id_tail,
    }
}

fn difference(difference: FileDifference) -> SyncDifference {
    SyncDifference {
        on_both_sides: difference.change.is_on_both_sides(),
        change: change(difference.change),
        remote_path: difference.remote_path,
    }
}

fn change(change: FileChange) -> SyncChange {
    match change {
        FileChange::ChangedLocally => SyncChange::ChangedLocally,
        FileChange::ChangedOnServer => SyncChange::ChangedOnServer,
        FileChange::ChangedOnBoth => SyncChange::ChangedOnBoth,
        FileChange::ChangedUnknown => SyncChange::Differs,
        FileChange::NewOnServer => SyncChange::NewOnServer,
        FileChange::DeletedLocally => SyncChange::DeletedLocally,
        FileChange::NewLocally => SyncChange::NewLocally,
        FileChange::DeletedOnServer => SyncChange::DeletedOnServer,
    }
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The protocol error for a failed Warp Sync operation; the message is what the user would have
/// seen in Warp. Some errors quote what the server printed, so it is made safe to show in a
/// terminal.
pub(super) fn control_error(error: WarpSyncError) -> ControlError {
    let code = match &error {
        WarpSyncError::InvalidPath(_) => ErrorCode::InvalidParams,
        WarpSyncError::NoSession(_) => ErrorCode::MissingTarget,
        WarpSyncError::AmbiguousSession(_) => ErrorCode::AmbiguousTarget,
        WarpSyncError::PendingNotFound => ErrorCode::StaleTarget,
        WarpSyncError::AlreadyInProgress => ErrorCode::TargetStateConflict,
        WarpSyncError::NotRemoteSession
        | WarpSyncError::UnsupportedShell
        | WarpSyncError::NotFound(_)
        | WarpSyncError::PermissionDenied { .. }
        | WarpSyncError::SpecialMode(_)
        | WarpSyncError::TooLarge { .. }
        | WarpSyncError::MissingTool(_)
        | WarpSyncError::Timeout
        | WarpSyncError::Executor(_)
        | WarpSyncError::RemoteCommandFailed { .. }
        | WarpSyncError::CorruptArchive(_)
        | WarpSyncError::UnexpectedArchiveEntry(_)
        | WarpSyncError::NotMirrored(_)
        | WarpSyncError::Manifest(_)
        | WarpSyncError::LocalIo(_)
        | WarpSyncError::NoEditor
        | WarpSyncError::Editor(_)
        | WarpSyncError::Baseline(_)
        | WarpSyncError::TooManyPending => ErrorCode::SyncFailed,
    };
    ControlError::new(code, printable(&error.to_string()))
}

#[cfg(test)]
#[path = "sync_reply_tests.rs"]
mod tests;
