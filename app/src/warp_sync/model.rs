use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use warpui::{Entity, ModelContext, SingletonEntity, WindowId};

use super::WarpSyncError;
use super::paths::{host_key, mirror_root};
use super::remote_script::ExtractMode;
use super::remote_shell::{RemoteShell, SessionShell};
use super::transfer::{
    DownloadOutcome, DownloadRequest, DownloadResult, PreparedUpload, UploadOutcome, UploadRequest,
    download, execute_upload, prepare_upload,
};
use crate::terminal::model::session::Session;

const BYTES_PER_KIB: u64 = 1024;
const BYTES_PER_MIB: u64 = 1024 * 1024;

/// Identifies an operation that is waiting for the user's confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PendingId(u64);

#[cfg(test)]
impl PendingId {
    pub(super) fn for_test(id: u64) -> Self {
        Self(id)
    }
}

/// What the user is asked to approve before an upload is sent.
#[derive(Debug, Clone)]
pub struct UploadSummary {
    pub remote_user: String,
    pub hostname: String,
    pub remote_path: String,
    pub files: usize,
    pub dirs: usize,
    pub content_bytes: u64,
    pub new_files: Vec<String>,
    pub missing_locally: Vec<String>,
    /// The remote `tar` is not GNU tar, so ownership may not be restored completely.
    pub ownership_may_be_incomplete: bool,
}

/// Progress of Warp Sync operations. Every event names the window that started the operation, so
/// that only that window reports it.
#[derive(Debug, Clone)]
pub enum WarpSyncEvent {
    Started {
        window_id: WindowId,
        description: String,
    },
    DownloadNeedsConfirmation {
        window_id: WindowId,
        id: PendingId,
        files: Vec<String>,
    },
    UploadNeedsConfirmation {
        window_id: WindowId,
        id: PendingId,
        summary: UploadSummary,
    },
    Succeeded {
        window_id: WindowId,
        message: String,
        open_path: Option<PathBuf>,
    },
    Failed {
        window_id: WindowId,
        error: WarpSyncError,
    },
}

/// A sync in progress is identified by its host and remote path. The key is held from the moment
/// an operation starts until it finishes or, when it waits for confirmation, until the user
/// answers.
type SyncKey = (String, String);

/// A validated session and the local locations for an operation that just started.
struct Begun {
    shell: Arc<dyn RemoteShell>,
    hostname: String,
    host_key: String,
    mirror_root: PathBuf,
}

struct PendingDownload {
    shell: Arc<dyn RemoteShell>,
    request: DownloadRequest,
    window_id: WindowId,
}

struct PendingUpload {
    shell: Arc<dyn RemoteShell>,
    prepared: PreparedUpload,
    window_id: WindowId,
}

#[derive(Default)]
pub struct WarpSyncModel {
    in_flight: HashSet<SyncKey>,
    pending_downloads: HashMap<PendingId, PendingDownload>,
    pending_uploads: HashMap<PendingId, PendingUpload>,
    next_pending_id: u64,
}

impl Entity for WarpSyncModel {
    type Event = WarpSyncEvent;
}

impl SingletonEntity for WarpSyncModel {}

impl WarpSyncModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_download(
        &mut self,
        session: Arc<Session>,
        remote_path: String,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        let begun = match self.begin(session, &remote_path) {
            Ok(begun) => begun,
            Err(error) => return ctx.emit(WarpSyncEvent::Failed { window_id, error }),
        };
        ctx.emit(WarpSyncEvent::Started {
            window_id,
            description: format!("Downloading {remote_path}…"),
        });
        let request = DownloadRequest {
            remote_path,
            host_key: begun.host_key,
            mirror_root: begun.mirror_root,
            allow_overwrite_local_changes: false,
        };
        self.spawn_download(begun.shell, request, window_id, ctx);
    }

    pub fn confirm_download_overwrite(&mut self, id: PendingId, ctx: &mut ModelContext<Self>) {
        let Some(pending) = self.pending_downloads.remove(&id) else {
            return;
        };
        let request = DownloadRequest {
            allow_overwrite_local_changes: true,
            ..pending.request
        };
        ctx.emit(WarpSyncEvent::Started {
            window_id: pending.window_id,
            description: format!("Downloading {}…", request.remote_path),
        });
        self.spawn_download(pending.shell, request, pending.window_id, ctx);
    }

    pub fn start_upload(
        &mut self,
        session: Arc<Session>,
        remote_path: String,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        let begun = match self.begin(session, &remote_path) {
            Ok(begun) => begun,
            Err(error) => return ctx.emit(WarpSyncEvent::Failed { window_id, error }),
        };
        ctx.emit(WarpSyncEvent::Started {
            window_id,
            description: format!("Preparing upload of {remote_path}…"),
        });

        let key = (begun.host_key.clone(), remote_path.clone());
        let request = UploadRequest {
            remote_path,
            host_key: begun.host_key,
            mirror_root: begun.mirror_root,
        };
        let shell = begun.shell;
        let hostname = begun.hostname;
        ctx.spawn(
            async move {
                let prepared = prepare_upload(shell.as_ref(), &request).await;
                (shell, prepared)
            },
            move |me, (shell, prepared), ctx| match prepared {
                Ok(prepared) => {
                    me.await_upload_confirmation(shell, hostname, prepared, window_id, ctx)
                }
                Err(error) => {
                    me.finish_sync(&key);
                    ctx.emit(WarpSyncEvent::Failed { window_id, error });
                }
            },
        );
    }

    pub fn confirm_upload(&mut self, id: PendingId, ctx: &mut ModelContext<Self>) {
        let Some(pending) = self.pending_uploads.remove(&id) else {
            return;
        };
        let window_id = pending.window_id;
        let PendingUpload {
            shell, prepared, ..
        } = pending;
        ctx.emit(WarpSyncEvent::Started {
            window_id,
            description: format!("Uploading {}…", prepared.remote_path),
        });

        let key = (prepared.host_key.clone(), prepared.remote_path.clone());
        ctx.spawn(
            async move {
                let outcome = execute_upload(shell.as_ref(), &prepared).await;
                (prepared.remote_path, outcome)
            },
            move |me, (remote_path, outcome), ctx| {
                me.finish_sync(&key);
                match outcome {
                    Ok(outcome) => ctx.emit(WarpSyncEvent::Succeeded {
                        window_id,
                        message: upload_message(&remote_path, &outcome),
                        open_path: None,
                    }),
                    Err(error) => ctx.emit(WarpSyncEvent::Failed { window_id, error }),
                }
            },
        );
    }

    /// Surfaces an error that occurred before an operation could be handed to the model.
    pub fn report_failure(
        &mut self,
        window_id: WindowId,
        error: WarpSyncError,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.emit(WarpSyncEvent::Failed { window_id, error });
    }

    /// Discards an operation that is waiting for confirmation. Nothing has been changed by then.
    pub fn cancel_pending(&mut self, id: PendingId) {
        if let Some(pending) = self.pending_downloads.remove(&id) {
            self.finish_sync(&(pending.request.host_key, pending.request.remote_path));
        }
        if let Some(pending) = self.pending_uploads.remove(&id) {
            self.finish_sync(&(pending.prepared.host_key, pending.prepared.remote_path));
        }
    }

    /// Validates the session and marks the path as in progress.
    fn begin(&mut self, session: Arc<Session>, remote_path: &str) -> Result<Begun, WarpSyncError> {
        let shell = SessionShell::new(session)?;
        let mirror_root = mirror_root().ok_or_else(|| {
            WarpSyncError::LocalIo("the home directory could not be determined".to_owned())
        })?;
        let host_key = host_key(shell.hostname());
        self.try_begin_sync(&host_key, remote_path)?;
        Ok(Begun {
            hostname: shell.hostname().to_owned(),
            shell: Arc::new(shell),
            host_key,
            mirror_root,
        })
    }

    /// Marks `remote_path` as in progress unless the same path, one of its ancestors, or one of
    /// its descendants is already being synced on that host.
    fn try_begin_sync(&mut self, host_key: &str, remote_path: &str) -> Result<(), WarpSyncError> {
        let conflicts = self
            .in_flight
            .iter()
            .any(|(in_flight_host, in_flight_path)| {
                in_flight_host == host_key && paths_overlap(in_flight_path, remote_path)
            });
        if conflicts {
            return Err(WarpSyncError::AlreadyInProgress);
        }
        self.in_flight
            .insert((host_key.to_owned(), remote_path.to_owned()));
        Ok(())
    }

    fn finish_sync(&mut self, key: &SyncKey) {
        self.in_flight.remove(key);
    }

    fn next_pending_id(&mut self) -> PendingId {
        self.next_pending_id += 1;
        PendingId(self.next_pending_id)
    }

    fn spawn_download(
        &mut self,
        shell: Arc<dyn RemoteShell>,
        request: DownloadRequest,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        let key = (request.host_key.clone(), request.remote_path.clone());
        ctx.spawn(
            async move {
                let result = download(shell.as_ref(), &request).await;
                (shell, request, result)
            },
            move |me, (shell, request, result), ctx| match result {
                Ok(DownloadResult::Done(outcome)) => {
                    me.finish_sync(&key);
                    ctx.emit(WarpSyncEvent::Succeeded {
                        window_id,
                        message: download_message(&request.remote_path, &outcome),
                        open_path: Some(outcome.local_path),
                    });
                }
                Ok(DownloadResult::NeedsConfirmation { modified_files }) => {
                    let id = me.next_pending_id();
                    me.pending_downloads.insert(
                        id,
                        PendingDownload {
                            shell,
                            request,
                            window_id,
                        },
                    );
                    ctx.emit(WarpSyncEvent::DownloadNeedsConfirmation {
                        window_id,
                        id,
                        files: modified_files,
                    });
                }
                Err(error) => {
                    me.finish_sync(&key);
                    ctx.emit(WarpSyncEvent::Failed { window_id, error });
                }
            },
        );
    }

    fn await_upload_confirmation(
        &mut self,
        shell: Arc<dyn RemoteShell>,
        hostname: String,
        prepared: PreparedUpload,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        let summary = UploadSummary {
            remote_user: prepared.probe.user.clone(),
            hostname,
            remote_path: prepared.remote_path.clone(),
            files: prepared.archive.files,
            dirs: prepared.archive.dirs,
            content_bytes: prepared.archive.content_bytes,
            new_files: prepared.archive.new_files.clone(),
            missing_locally: prepared.archive.missing_locally.clone(),
            ownership_may_be_incomplete: ExtractMode::for_probe(&prepared.probe)
                == ExtractMode::Generic,
        };
        let id = self.next_pending_id();
        self.pending_uploads.insert(
            id,
            PendingUpload {
                shell,
                prepared,
                window_id,
            },
        );
        ctx.emit(WarpSyncEvent::UploadNeedsConfirmation {
            window_id,
            id,
            summary,
        });
    }
}

/// Whether two normalized absolute paths are the same or one contains the other.
fn paths_overlap(a: &str, b: &str) -> bool {
    let contains = |outer: &str, inner: &str| {
        inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    contains(a, b) || contains(b, a)
}

fn download_message(remote_path: &str, outcome: &DownloadOutcome) -> String {
    let mut message = format!(
        "Downloaded {} ({}) from {remote_path} as {}",
        pluralize_count(outcome.files, "file"),
        format_size(outcome.total_bytes),
        outcome.remote_user
    );
    if !outcome.skipped.is_empty() {
        message.push_str(&format!(
            ". Skipped {} link(s) and special file(s)",
            outcome.skipped.len()
        ));
    }
    message
}

fn upload_message(remote_path: &str, outcome: &UploadOutcome) -> String {
    let mut message = format!(
        "Uploaded {} to {remote_path} as {}",
        pluralize_count(outcome.files, "file"),
        outcome.remote_user
    );
    if let Some(backup_path) = &outcome.backup_path {
        message.push_str(&format!(". Previous version saved to {backup_path}"));
    }
    message
}

pub(super) fn pluralize_count(count: usize, noun: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{suffix}")
}

pub(super) fn format_size(bytes: u64) -> String {
    if bytes >= BYTES_PER_MIB {
        format!("{:.1} MiB", bytes as f64 / BYTES_PER_MIB as f64)
    } else if bytes >= BYTES_PER_KIB {
        format!("{:.1} KiB", bytes as f64 / BYTES_PER_KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
