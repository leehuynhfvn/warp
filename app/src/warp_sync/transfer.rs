//! Orchestrates a download or upload between the remote host and the local mirror. Everything
//! here is `async` and blocking-IO heavy, so callers run it on a background executor.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use warp_core::{safe_info, safe_warn};

use super::archive::{
    build_upload, extract_download, locally_modified_files, SkipReason, UploadArchive,
};
use super::manifest::{Manifest, SyncRecord};
use super::paths::{local_path_for, manifest_path, split_parent_name, staging_dir};
use super::remote_script::{
    cleanup_command, download_script, parse_probe_output, probe_script, upload_begin_command,
    upload_chunk_commands, upload_commit_script, validate_tmp_dir, wrap_for_any_shell, ExtractMode,
    ProbeResult, ProbeStatus, RemoteTmpDir, UploadCommit,
};
use super::remote_shell::RemoteShell;
use super::{WarpSyncError, MAX_DOWNLOAD_KIB};

const MAX_BACKUP_STEM_CHARS: usize = 150;

/// Where the previous local copy is parked, inside the staging directory, while the new one is
/// moved into place.
const PREVIOUS_COPY_DIR: &str = "previous";

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    /// Normalized absolute remote path.
    pub remote_path: String,
    pub host_key: String,
    pub mirror_root: PathBuf,
    /// Whether local edits under the path may be overwritten.
    pub allow_overwrite_local_changes: bool,
}

#[derive(Debug)]
pub struct DownloadOutcome {
    pub local_path: PathBuf,
    pub files: usize,
    pub dirs: usize,
    pub total_bytes: u64,
    pub skipped: Vec<(String, SkipReason)>,
    pub remote_user: String,
}

#[derive(Debug)]
pub enum DownloadResult {
    Done(DownloadOutcome),
    /// Nothing was changed: replacing the mirror would discard these local edits.
    NeedsConfirmation { modified_files: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct UploadRequest {
    /// Normalized absolute remote path.
    pub remote_path: String,
    pub host_key: String,
    pub mirror_root: PathBuf,
}

/// An upload that has been packed and checked against the remote host but not sent yet.
#[derive(Debug)]
pub struct PreparedUpload {
    pub archive: UploadArchive,
    pub probe: ProbeResult,
    pub remote_path: String,
    pub host_key: String,
    pub mirror_root: PathBuf,
}

#[derive(Debug)]
pub struct UploadOutcome {
    pub files: usize,
    pub dirs: usize,
    pub content_bytes: u64,
    /// Remote path of the backup of what was overwritten, if anything was.
    pub backup_path: Option<String>,
    pub remote_user: String,
}

pub async fn download(
    shell: &dyn RemoteShell,
    request: &DownloadRequest,
) -> Result<DownloadResult, WarpSyncError> {
    let probe = probe(shell, &request.remote_path).await?;
    ensure_readable(&probe, &request.remote_path)?;
    ensure_download_size(&probe)?;

    let manifest = load_manifest(&request.mirror_root, &request.host_key)?;
    if !request.allow_overwrite_local_changes {
        let modified_files = locally_modified_files(
            &request.remote_path,
            &manifest.entries_under(&request.remote_path),
            &request.mirror_root,
            &request.host_key,
        )?;
        if !modified_files.is_empty() {
            return Ok(DownloadResult::NeedsConfirmation { modified_files });
        }
    }

    let (parent, name) = split_parent_name(&request.remote_path);
    let tgz = shell
        .run(&wrap_for_any_shell(&download_script(&parent, &name)))
        .await?;

    let staging = staging_dir(&request.mirror_root);
    let applied = apply_download(&tgz, request, &probe, manifest, &staging);
    remove_staging(&staging);
    let outcome = applied?;
    safe_info!(
        safe: ("Warp Sync: downloaded {} files", outcome.files),
        full: ("Warp Sync: downloaded {} files from {}", outcome.files, request.remote_path)
    );
    Ok(DownloadResult::Done(outcome))
}

pub async fn prepare_upload(
    shell: &dyn RemoteShell,
    request: &UploadRequest,
) -> Result<PreparedUpload, WarpSyncError> {
    let manifest = load_manifest(&request.mirror_root, &request.host_key)?;
    let archive = build_upload(
        &request.remote_path,
        &manifest,
        &request.mirror_root,
        &request.host_key,
    )?;

    let probe = probe(shell, &request.remote_path).await?;
    ensure_readable(&probe, &request.remote_path)?;
    if !probe.has_base64 {
        return Err(WarpSyncError::MissingTool("base64"));
    }
    Ok(PreparedUpload {
        archive,
        probe,
        remote_path: request.remote_path.clone(),
        host_key: request.host_key.clone(),
        mirror_root: request.mirror_root.clone(),
    })
}

pub async fn execute_upload(
    shell: &dyn RemoteShell,
    prepared: &PreparedUpload,
) -> Result<UploadOutcome, WarpSyncError> {
    let begin = shell.run(&upload_begin_command()).await?;
    let tmp_dir = validate_tmp_dir(&String::from_utf8_lossy(&begin))?;

    let backup_path = match send_and_commit(shell, prepared, &tmp_dir).await {
        Ok(backup_path) => backup_path,
        Err(err) => {
            remove_remote_tmp_dir(shell, &tmp_dir).await;
            return Err(err);
        }
    };
    record_upload(prepared).map_err(|err| {
        WarpSyncError::Manifest(format!(
            "the upload succeeded but the local record could not be updated: {err}"
        ))
    })?;

    safe_info!(
        safe: ("Warp Sync: uploaded {} files", prepared.archive.files),
        full: ("Warp Sync: uploaded {} files to {}", prepared.archive.files, prepared.remote_path)
    );
    Ok(UploadOutcome {
        files: prepared.archive.files,
        dirs: prepared.archive.dirs,
        content_bytes: prepared.archive.content_bytes,
        backup_path,
        remote_user: prepared.probe.user.clone(),
    })
}

async fn probe(shell: &dyn RemoteShell, remote_path: &str) -> Result<ProbeResult, WarpSyncError> {
    let output = shell
        .run(&wrap_for_any_shell(&probe_script(remote_path)))
        .await?;
    parse_probe_output(&String::from_utf8_lossy(&output))
}

fn ensure_readable(probe: &ProbeResult, remote_path: &str) -> Result<(), WarpSyncError> {
    match probe.status {
        ProbeStatus::Ok => Ok(()),
        ProbeStatus::NotFound => Err(WarpSyncError::NotFound(remote_path.to_owned())),
        ProbeStatus::PermissionDenied => Err(WarpSyncError::PermissionDenied {
            user: probe.user.clone(),
        }),
    }
}

fn ensure_download_size(probe: &ProbeResult) -> Result<(), WarpSyncError> {
    match probe.size_kib {
        Some(size_kib) if size_kib > MAX_DOWNLOAD_KIB => Err(WarpSyncError::TooLarge {
            limit_desc: format!(
                "{size_kib} KiB on the remote host; the limit is {MAX_DOWNLOAD_KIB} KiB"
            ),
        }),
        Some(_) | None => Ok(()),
    }
}

fn load_manifest(mirror_root: &Path, host_key: &str) -> Result<Manifest, WarpSyncError> {
    Manifest::load_or_default(&manifest_path(mirror_root, host_key), host_key)
}

fn apply_download(
    tgz: &[u8],
    request: &DownloadRequest,
    probe: &ProbeResult,
    mut manifest: Manifest,
    staging: &Path,
) -> Result<DownloadOutcome, WarpSyncError> {
    let (parent, name) = split_parent_name(&request.remote_path);
    let report = extract_download(tgz, &name, &parent, staging)?;
    if report.entries.is_empty() {
        return Err(WarpSyncError::InvalidPath(format!(
            "{} is a symbolic link or special file, which Warp Sync skips",
            request.remote_path
        )));
    }

    let local_path = local_path_for(&request.mirror_root, &request.host_key, &request.remote_path);
    swap_into_place(
        &staging.join(&name),
        &local_path,
        &staging.join(PREVIOUS_COPY_DIR),
    )?;

    manifest.replace_subtree(&request.remote_path, report.entries);
    manifest.record_sync(
        &request.remote_path,
        SyncRecord {
            remote_user: probe.user.clone(),
            at_unix: now_unix(),
        },
    );
    manifest.save_atomic(&manifest_path(&request.mirror_root, &request.host_key))?;

    Ok(DownloadOutcome {
        local_path,
        files: report.files,
        dirs: report.dirs,
        total_bytes: report.total_bytes,
        skipped: report.skipped,
        remote_user: probe.user.clone(),
    })
}

/// Replaces `target` with `new`, keeping the old copy in `previous` until the move succeeds so
/// that a failure cannot lose the mirror.
fn swap_into_place(new: &Path, target: &Path, previous: &Path) -> Result<(), WarpSyncError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|err| local_io("create", parent, &err))?;
    }
    let had_previous = match fs::symlink_metadata(target) {
        Ok(_) => true,
        Err(err) if err.kind() == ErrorKind::NotFound => false,
        Err(err) => return Err(local_io("read", target, &err)),
    };
    if had_previous {
        fs::rename(target, previous).map_err(|err| local_io("move", target, &err))?;
    }
    if let Err(err) = fs::rename(new, target) {
        if had_previous && let Err(restore_err) = fs::rename(previous, target) {
            safe_warn!(
                safe: ("Warp Sync: could not restore the previous mirror: {restore_err}"),
                full: ("Warp Sync: could not restore {} from {}: {restore_err}",
                    target.display(), previous.display())
            );
        }
        return Err(local_io("move into place", target, &err));
    }
    Ok(())
}

fn remove_staging(staging: &Path) {
    match fs::remove_dir_all(staging) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => safe_warn!(
            safe: ("Warp Sync: could not remove the staging directory: {err}"),
            full: ("Warp Sync: could not remove {}: {err}", staging.display())
        ),
    }
}

/// Sends the payload and runs the commit script. Returns the remote path of the backup, if one
/// was made.
async fn send_and_commit(
    shell: &dyn RemoteShell,
    prepared: &PreparedUpload,
    tmp_dir: &RemoteTmpDir,
) -> Result<Option<String>, WarpSyncError> {
    for command in upload_chunk_commands(tmp_dir, &prepared.archive.bytes) {
        shell.run(&command).await?;
    }

    let (parent, name) = split_parent_name(&prepared.remote_path);
    let backup_name = backup_name(&prepared.host_key, &prepared.remote_path, now_unix());
    let script = upload_commit_script(&UploadCommit {
        tmp_dir,
        parent: &parent,
        name: &name,
        expected_len: prepared.archive.bytes.len(),
        backup_name: &backup_name,
        extract_mode: ExtractMode::for_probe(&prepared.probe),
    });
    let output = shell.run(&wrap_for_any_shell(&script)).await?;
    Ok(parse_backup_path(&String::from_utf8_lossy(&output)))
}

async fn remove_remote_tmp_dir(shell: &dyn RemoteShell, tmp_dir: &RemoteTmpDir) {
    if let Err(err) = shell.run(&cleanup_command(tmp_dir)).await {
        safe_warn!(
            safe: ("Warp Sync: could not remove the remote scratch directory: {err}"),
            full: ("Warp Sync: could not remove {}: {err}", tmp_dir.as_str())
        );
    }
}

/// Updates the manifest with what was just uploaded. Entries that the mirror no longer has are
/// kept, since they still exist on the remote host.
fn record_upload(prepared: &PreparedUpload) -> Result<(), WarpSyncError> {
    let (parent, name) = split_parent_name(&prepared.remote_path);
    let staging = staging_dir(&prepared.mirror_root);
    let report = extract_download(&prepared.archive.bytes, &name, &parent, &staging);
    remove_staging(&staging);
    let report = report?;

    let mut manifest = load_manifest(&prepared.mirror_root, &prepared.host_key)?;
    manifest.upsert_entries(report.entries);
    manifest.record_sync(
        &prepared.remote_path,
        SyncRecord {
            remote_user: prepared.probe.user.clone(),
            at_unix: now_unix(),
        },
    );
    manifest.save_atomic(&manifest_path(&prepared.mirror_root, &prepared.host_key))
}

/// A file-name stem that identifies the host, the path and the time, using only characters that
/// are safe in the commit script.
fn backup_name(host_key: &str, remote_path: &str, at_unix: u64) -> String {
    let path_part: String = remote_path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem: String = format!("{host_key}{path_part}")
        .chars()
        .take(MAX_BACKUP_STEM_CHARS)
        .collect();
    format!("{stem}-{at_unix}")
}

fn parse_backup_path(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("backup="))
        .map(str::to_owned)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn local_io(action: &str, path: &Path, err: &std::io::Error) -> WarpSyncError {
    WarpSyncError::LocalIo(format!("could not {action} {}: {err}", path.display()))
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
