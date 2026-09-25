//! Who asked for a Warp Sync operation, and how its outcome travels back to them.

use std::path::PathBuf;

use futures::channel::oneshot;
use uuid::Uuid;
use warpui::WindowId;

use super::WarpSyncError;
use super::diff::FileDifference;
use super::model::UploadSummary;
use super::paths::printable;
use super::remote_check::{RemoteCheck, RemoteConflicts};
use super::transfer::{CompareOutcome, DownloadOutcome, UploadOutcome};

/// The party that started an operation.
pub enum Requester {
    /// A Warp window: progress and results are events that the window turns into toasts and
    /// dialogs.
    Window(WindowId),
    /// A local-control client that has no window of its own: results are handed back as a
    /// [`SyncReply`], and the user only sees the operation start in `window_id`, the window whose
    /// shell runs the commands.
    External {
        reply: ExternalReply,
        window_id: WindowId,
        /// The mirror folder the client's path belongs to. Only consulted when an operation
        /// starts: the operation fails if the session's machine owns a different mirror.
        expected_host_key: Option<String>,
    },
}

impl From<WindowId> for Requester {
    fn from(window_id: WindowId) -> Self {
        Self::Window(window_id)
    }
}

impl Requester {
    pub fn external(
        window_id: WindowId,
        expected_host_key: Option<String>,
    ) -> (Self, oneshot::Receiver<Result<SyncReply, WarpSyncError>>) {
        let (reply, receiver) = ExternalReply::channel();
        let requester = Self::External {
            reply,
            window_id,
            expected_host_key,
        };
        (requester, receiver)
    }

    pub(super) fn window_id(&self) -> WindowId {
        match self {
            Self::Window(window_id) | Self::External { window_id, .. } => *window_id,
        }
    }

    pub(super) fn expected_host_key(&self) -> Option<String> {
        match self {
            Self::Window(_) => None,
            Self::External {
                expected_host_key, ..
            } => expected_host_key.clone(),
        }
    }
}

/// The single answer owed to a local-control client.
pub struct ExternalReply {
    sender: oneshot::Sender<Result<SyncReply, WarpSyncError>>,
}

impl ExternalReply {
    pub fn channel() -> (Self, oneshot::Receiver<Result<SyncReply, WarpSyncError>>) {
        let (sender, receiver) = oneshot::channel();
        (Self { sender }, receiver)
    }

    pub(super) fn send(self, result: Result<SyncReply, WarpSyncError>) {
        if self.sender.send(result).is_err() {
            log::debug!("A local-control client stopped waiting for its Warp Sync reply");
        }
    }
}

/// What an external client is told about an operation. Every string that originates on the
/// server has been made safe to print.
#[derive(Debug, Clone)]
pub enum SyncReply {
    Downloaded {
        local_path: PathBuf,
        files: usize,
        dirs: usize,
        bytes: u64,
        remote_user: String,
        skipped: Vec<SkippedEntry>,
        baseline_warning: Option<String>,
    },
    /// Nothing has been changed yet: the client must answer with the confirm or cancel call that
    /// carries `pending_id`.
    NeedsConfirmation {
        pending_id: Uuid,
        kind: ConfirmationKind,
    },
    Uploaded {
        files: usize,
        dirs: usize,
        bytes: u64,
        remote_user: String,
        backup_path: Option<String>,
        baseline_warning: Option<String>,
    },
    Compared {
        differences: Vec<FileDifference>,
        identical_files: usize,
        diff_path: PathBuf,
        host_dir: PathBuf,
        server_copy_dir: PathBuf,
        remote_user: String,
    },
    Unchanged {
        identical_files: usize,
    },
}

#[derive(Debug, Clone)]
pub struct SkippedEntry {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum ConfirmationKind {
    OverwriteLocalChanges { files: Vec<String> },
    Upload(Box<UploadSummary>),
}

/// How an operation ended, before it is shaped for whoever asked.
pub(super) enum Finished {
    Downloaded {
        remote_path: String,
        outcome: DownloadOutcome,
    },
    Uploaded {
        remote_path: String,
        outcome: UploadOutcome,
    },
    Compared {
        hostname: String,
        remote_path: String,
        outcome: CompareOutcome,
    },
    Failed(WarpSyncError),
}

impl Finished {
    pub(super) fn into_reply(self) -> Result<SyncReply, WarpSyncError> {
        match self {
            Self::Downloaded { outcome, .. } => Ok(SyncReply::Downloaded {
                local_path: outcome.local_path,
                files: outcome.files,
                dirs: outcome.dirs,
                bytes: outcome.total_bytes,
                remote_user: printable(&outcome.remote_user),
                skipped: outcome
                    .skipped
                    .into_iter()
                    .map(|(path, reason)| SkippedEntry {
                        path: printable(&path),
                        reason: reason.to_string(),
                    })
                    .collect(),
                baseline_warning: outcome.baseline_warning.as_deref().map(printable),
            }),
            Self::Uploaded { outcome, .. } => Ok(SyncReply::Uploaded {
                files: outcome.files,
                dirs: outcome.dirs,
                bytes: outcome.content_bytes,
                remote_user: printable(&outcome.remote_user),
                backup_path: outcome.backup_path.as_deref().map(printable),
                baseline_warning: outcome.baseline_warning.as_deref().map(printable),
            }),
            Self::Compared { outcome, .. } => Ok(match outcome.diff_path {
                Some(diff_path) => SyncReply::Compared {
                    differences: outcome
                        .differences
                        .into_iter()
                        .map(|difference| FileDifference {
                            remote_path: printable(&difference.remote_path),
                            ..difference
                        })
                        .collect(),
                    identical_files: outcome.identical_files,
                    diff_path,
                    host_dir: outcome.host_dir,
                    server_copy_dir: outcome.server_copy_dir,
                    remote_user: printable(&outcome.remote_user),
                },
                None => SyncReply::Unchanged {
                    identical_files: outcome.identical_files,
                },
            }),
            Self::Failed(error) => Err(error),
        }
    }
}

impl ConfirmationKind {
    pub(super) fn overwrite_local_changes(files: Vec<String>) -> Self {
        Self::OverwriteLocalChanges {
            files: printable_all(files),
        }
    }

    pub(super) fn upload(summary: UploadSummary) -> Self {
        Self::Upload(Box::new(UploadSummary {
            remote_user: printable(&summary.remote_user),
            hostname: printable(&summary.hostname),
            remote_path: printable(&summary.remote_path),
            new_files: printable_all(summary.new_files),
            new_modes: summary
                .new_modes
                .into_iter()
                .map(|(path, mode)| (printable(&path), mode))
                .collect(),
            creates_under: summary.creates_under.as_deref().map(printable),
            missing_locally: printable_all(summary.missing_locally),
            remote_check: printable_check(summary.remote_check),
            server_id_tail: summary.server_id_tail.as_deref().map(printable),
            ..summary
        }))
    }
}

fn printable_all(paths: Vec<String>) -> Vec<String> {
    paths.iter().map(|path| printable(path)).collect()
}

fn printable_check(check: RemoteCheck) -> RemoteCheck {
    match check {
        RemoteCheck::Unavailable => RemoteCheck::Unavailable,
        RemoteCheck::Checked(conflicts) => RemoteCheck::Checked(RemoteConflicts {
            changed: printable_all(conflicts.changed),
            missing: printable_all(conflicts.missing),
            already_exist: printable_all(conflicts.already_exist),
        }),
    }
}

#[cfg(test)]
#[path = "requester_tests.rs"]
mod tests;
