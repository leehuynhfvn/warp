use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;
use warpui::r#async::Timer;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity, WindowId};

use super::config::{SyncConfig, SyncLimits};
use super::diff::FileDifference;
use super::editor::{EditorCli, EditorRequest, MAX_EDITOR_DIFFS, launch};
use super::manifest::Manifest;
use super::paths::{host_key, manifest_path, printable};
use super::remote_check::RemoteCheck;
use super::remote_script::ExtractMode;
use super::remote_shell::{RemoteShell, SessionShell};
use super::requester::{
    ConfirmationKind, ExternalReply, Finished, RemoteEditStep, Requester, SyncReply,
};
use super::risk::{UploadRisks, assess};
use super::transfer::{
    CompareOutcome, CompareRequest, DownloadOutcome, DownloadRequest, DownloadResult,
    PreparedUpload, UploadOutcome, UploadPlacement, UploadRequest, compare, download,
    execute_upload, prepare_upload,
};
use super::{EXTERNAL_PENDING_TTL, MAX_EXTERNAL_PENDING, WarpSyncError};
use crate::host_directory::{self, HostDirectoryModel, MirrorLink};
use crate::terminal::model::session::Session;

/// Marks toasts that report an operation started by a local-control client rather than by the
/// window's own user.
const EXTERNAL_TOAST_PREFIX: &str = "Warp Sync (local control): ";
const BYTES_PER_KIB: u64 = 1024;
const SERVER_ID_TAIL_LEN: usize = 4;
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
    /// The permission bits each of `new_files` will get on the remote host.
    pub new_modes: BTreeMap<String, u32>,
    /// The synced directory that the path is created in, when the path is not on the remote host
    /// yet. Nothing is replaced then.
    pub creates_under: Option<String>,
    /// New entries that deserve a warning before the user goes ahead.
    pub risks: UploadRisks,
    pub missing_locally: Vec<String>,
    pub remote_check: RemoteCheck,
    /// The remote `tar` is not GNU tar, so ownership may not be restored completely.
    pub ownership_may_be_incomplete: bool,
    /// The last characters of the remote machine id, to tell apart hosts with the same name.
    pub server_id_tail: Option<String>,
    /// What the upload changes, when it replaces a single text file.
    pub diff: Option<Vec<String>>,
}

/// What a comparison of the local mirror with the remote host found. Only produced when there is
/// at least one difference.
#[derive(Debug, Clone)]
pub struct CompareSummary {
    pub remote_user: String,
    pub hostname: String,
    pub remote_path: String,
    /// Sorted by remote path.
    pub differences: Vec<FileDifference>,
    pub identical_files: usize,
    /// The written comparison.
    pub diff_path: PathBuf,
    /// The mirror of the whole host.
    pub host_dir: PathBuf,
    /// Where the server's copy is kept, laid out like `host_dir`.
    pub server_copy_dir: PathBuf,
}

impl CompareSummary {
    /// Opens the files that both sides have side by side, server first. The written comparison is
    /// opened too when some differences cannot be shown that way.
    pub fn editor_request(&self) -> EditorRequest {
        let (on_both_sides, on_one_side): (Vec<&FileDifference>, Vec<&FileDifference>) = self
            .differences
            .iter()
            .partition(|difference| difference.change.is_on_both_sides());
        let diffs = on_both_sides
            .iter()
            .take(MAX_EDITOR_DIFFS)
            .map(|difference| {
                let relative = difference.remote_path.trim_start_matches('/');
                (
                    self.server_copy_dir.join(relative),
                    self.host_dir.join(relative),
                )
            })
            .collect();
        let shows_everything = on_one_side.is_empty() && on_both_sides.len() <= MAX_EDITOR_DIFFS;
        EditorRequest::OpenDiffs {
            workspace: self.host_dir.clone(),
            diffs,
            files: if shows_everything {
                Vec::new()
            } else {
                vec![self.diff_path.clone()]
            },
        }
    }

    /// One line that says where the comparison ended up, for when the summary cannot be shown in
    /// a dialog.
    pub fn announcement(&self) -> String {
        format!(
            "Compared {}: {}. The diff is saved at {}",
            printable(&self.remote_path),
            pluralize_count(self.differences.len(), "difference"),
            self.diff_path.display()
        )
    }
}

/// A path in the local mirror together with the mirror of its whole host.
#[derive(Debug, Clone)]
pub struct MirrorLocation {
    pub host_dir: PathBuf,
    pub local_path: PathBuf,
    pub is_file: bool,
}

/// How an operation that edits a remote file in Warp's editor ended. Failures have already been
/// reported to the user by then.
#[derive(Debug, Clone)]
pub enum RemoteEditOutcome {
    Downloaded {
        local_path: PathBuf,
        remote_path: String,
        remote_user: String,
    },
    Uploaded,
    Failed(String),
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
        summary: Box<UploadSummary>,
    },
    CompareFinished {
        window_id: WindowId,
        summary: Box<CompareSummary>,
    },
    Succeeded {
        window_id: WindowId,
        message: String,
        /// Where the result can be opened.
        location: Option<MirrorLocation>,
    },
    Failed {
        window_id: WindowId,
        error: WarpSyncError,
    },
    /// Follows the event that reported an operation a [`Requester::RemoteEdit`] started.
    RemoteEditFinished {
        window_id: WindowId,
        step: RemoteEditStep,
        outcome: RemoteEditOutcome,
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
    limits: SyncLimits,
}

struct PendingDownload {
    shell: Arc<dyn RemoteShell>,
    request: DownloadRequest,
    window_id: WindowId,
    edit: Option<RemoteEditStep>,
}

struct PendingUpload {
    shell: Arc<dyn RemoteShell>,
    prepared: PreparedUpload,
    /// The key that `begin` took, which is not `prepared.host_key`: that one names the mirror
    /// folder of the machine that was found, and may carry a machine suffix.
    key: SyncKey,
    window_id: WindowId,
    edit: Option<RemoteEditStep>,
}

/// An operation waiting for a local-control client's answer. It is kept apart from the operations
/// that wait for a dialog, so that a client can neither answer a dialog nor be answered by one.
enum ExternalPending {
    Download(PendingDownload),
    Upload(Box<PendingUpload>),
}

impl ExternalPending {
    fn key(&self) -> SyncKey {
        match self {
            Self::Download(pending) => (
                pending.request.host_key.clone(),
                pending.request.remote_path.clone(),
            ),
            Self::Upload(pending) => pending.key.clone(),
        }
    }
}

pub struct WarpSyncModel {
    in_flight: HashSet<SyncKey>,
    pending_downloads: HashMap<PendingId, PendingDownload>,
    pending_uploads: HashMap<PendingId, PendingUpload>,
    next_pending_id: u64,
    /// Keyed by an unguessable id: the client that receives it is the only one that can answer.
    external_pending: HashMap<Uuid, ExternalPending>,
    external_pending_ttl: Duration,
}

impl Default for WarpSyncModel {
    fn default() -> Self {
        Self::with_external_pending_ttl(EXTERNAL_PENDING_TTL)
    }
}

impl Entity for WarpSyncModel {
    type Event = WarpSyncEvent;
}

impl SingletonEntity for WarpSyncModel {}

impl WarpSyncModel {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_external_pending_ttl(external_pending_ttl: Duration) -> Self {
        Self {
            in_flight: HashSet::new(),
            pending_downloads: HashMap::new(),
            pending_uploads: HashMap::new(),
            next_pending_id: 0,
            external_pending: HashMap::new(),
            external_pending_ttl,
        }
    }

    pub fn start_download(
        &mut self,
        session: Arc<Session>,
        remote_path: String,
        requester: impl Into<Requester>,
        ctx: &mut ModelContext<Self>,
    ) {
        let requester = requester.into();
        let begun = match self.begin(session, &remote_path, ctx) {
            Ok(begun) => begun,
            Err(error) => return report(requester, Finished::Failed(error), ctx),
        };
        announce(&requester, format!("Downloading {remote_path}…"), ctx);
        let request = DownloadRequest {
            remote_path,
            host_key: begun.host_key,
            expected_host_key: requester.expected_host_key(),
            mirror_root: begun.mirror_root,
            limits: begun.limits,
            allow_overwrite_local_changes: false,
            require_file: matches!(requester.edit_step(), Some(RemoteEditStep::Open { .. })),
        };
        self.spawn_download(begun.shell, request, requester, ctx);
    }

    pub fn confirm_download_overwrite(&mut self, id: PendingId, ctx: &mut ModelContext<Self>) {
        let Some(pending) = self.pending_downloads.remove(&id) else {
            return;
        };
        let requester = Requester::for_window(pending.window_id, pending.edit.clone());
        self.resume_download(pending, requester, ctx);
    }

    pub fn start_upload(
        &mut self,
        session: Arc<Session>,
        remote_path: String,
        requester: impl Into<Requester>,
        ctx: &mut ModelContext<Self>,
    ) {
        let requester = requester.into();
        let begun = match self.begin(session, &remote_path, ctx) {
            Ok(begun) => begun,
            Err(error) => return report(requester, Finished::Failed(error), ctx),
        };
        announce(
            &requester,
            format!("Preparing upload of {remote_path}…"),
            ctx,
        );

        let key = (begun.host_key.clone(), remote_path.clone());
        let request = UploadRequest {
            remote_path,
            host_key: begun.host_key,
            expected_host_key: requester.expected_host_key(),
            mirror_root: begun.mirror_root,
            limits: begun.limits,
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
                    me.await_upload_confirmation(shell, hostname, prepared, key, requester, ctx)
                }
                Err(error) => {
                    me.finish_sync(&key);
                    report(requester, Finished::Failed(error), ctx);
                }
            },
        );
    }

    pub fn start_compare(
        &mut self,
        session: Arc<Session>,
        remote_path: String,
        requester: impl Into<Requester>,
        ctx: &mut ModelContext<Self>,
    ) {
        let requester = requester.into();
        let begun = match self.begin(session, &remote_path, ctx) {
            Ok(begun) => begun,
            Err(error) => return report(requester, Finished::Failed(error), ctx),
        };
        announce(
            &requester,
            format!("Comparing {remote_path} with the local mirror…"),
            ctx,
        );

        let key = (begun.host_key.clone(), remote_path.clone());
        let request = CompareRequest {
            remote_path,
            hostname: begun.hostname.clone(),
            host_key: begun.host_key,
            expected_host_key: requester.expected_host_key(),
            mirror_root: begun.mirror_root,
            limits: begun.limits,
        };
        let shell = begun.shell;
        let hostname = begun.hostname;
        ctx.spawn(
            async move {
                let outcome = compare(shell.as_ref(), &request).await;
                (request.remote_path, outcome)
            },
            move |me, (remote_path, outcome), ctx| {
                me.finish_sync(&key);
                let finished = match outcome {
                    Ok(outcome) => Finished::Compared {
                        hostname,
                        remote_path,
                        outcome,
                    },
                    Err(error) => Finished::Failed(error),
                };
                report(requester, finished, ctx);
            },
        );
    }

    pub fn confirm_upload(&mut self, id: PendingId, ctx: &mut ModelContext<Self>) {
        let Some(pending) = self.pending_uploads.remove(&id) else {
            return;
        };
        let requester = Requester::for_window(pending.window_id, pending.edit.clone());
        self.resume_upload(pending, requester, ctx);
    }

    /// Carries on with the operation that a local-control client was asked to confirm.
    pub fn confirm_external(
        &mut self,
        id: Uuid,
        reply: ExternalReply,
        ctx: &mut ModelContext<Self>,
    ) {
        match self.external_pending.remove(&id) {
            Some(ExternalPending::Download(pending)) => {
                let window_id = pending.window_id;
                let requester = Requester::External {
                    reply,
                    window_id,
                    expected_host_key: None,
                };
                self.resume_download(pending, requester, ctx);
            }
            Some(ExternalPending::Upload(pending)) => {
                let window_id = pending.window_id;
                let requester = Requester::External {
                    reply,
                    window_id,
                    expected_host_key: None,
                };
                self.resume_upload(*pending, requester, ctx);
            }
            None => reply.send(Err(WarpSyncError::PendingNotFound)),
        }
    }

    /// Discards an operation that a local-control client was asked to confirm.
    pub fn cancel_external(&mut self, id: Uuid) -> Result<(), WarpSyncError> {
        if self.discard_external(id) {
            Ok(())
        } else {
            Err(WarpSyncError::PendingNotFound)
        }
    }

    /// Opens `request` in the editor chosen for opening file links, or in VS Code.
    pub fn open_in_editor(
        &mut self,
        request: EditorRequest,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        let cli = EditorCli::from_settings_or_vs_code(ctx);
        ctx.spawn(
            async move { launch(cli, &request) },
            move |_, launched, ctx| {
                if let Err(error) = launched {
                    ctx.emit(WarpSyncEvent::Failed { window_id, error });
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
    /// Returns the editing step the operation carried out, if it was started from Warp's editor.
    pub fn cancel_pending(&mut self, id: PendingId) -> Option<RemoteEditStep> {
        if let Some(pending) = self.pending_downloads.remove(&id) {
            self.finish_sync(&(pending.request.host_key, pending.request.remote_path));
            return pending.edit;
        }
        if let Some(pending) = self.pending_uploads.remove(&id) {
            self.finish_sync(&pending.key);
            return pending.edit;
        }
        None
    }

    /// Validates the session and marks the path as in progress.
    fn begin(
        &mut self,
        session: Arc<Session>,
        remote_path: &str,
        ctx: &AppContext,
    ) -> Result<Begun, WarpSyncError> {
        let shell = SessionShell::new(session)?;
        let SyncConfig {
            mirror_root,
            limits,
        } = SyncConfig::from_settings(ctx)?;
        let host_key = host_key(shell.hostname());
        self.try_begin_sync(&host_key, remote_path)?;
        Ok(Begun {
            hostname: shell.hostname().to_owned(),
            shell: Arc::new(shell),
            host_key,
            mirror_root,
            limits,
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

    /// Holds `pending` for a local-control client until it answers or the hold expires. Returns
    /// the id the client must present. When too many operations are already held, the path is
    /// released and the operation is refused.
    fn hold_external(
        &mut self,
        pending: ExternalPending,
        ctx: &mut ModelContext<Self>,
    ) -> Result<Uuid, WarpSyncError> {
        if self.external_pending.len() >= MAX_EXTERNAL_PENDING {
            self.finish_sync(&pending.key());
            return Err(WarpSyncError::TooManyPending);
        }
        let id = Uuid::new_v4();
        self.external_pending.insert(id, pending);
        let ttl = self.external_pending_ttl;
        ctx.spawn(async move { Timer::after(ttl).await }, move |me, _, _| {
            me.discard_external(id);
        });
        Ok(id)
    }

    /// Drops a held operation and releases its path. Returns whether there was one.
    fn discard_external(&mut self, id: Uuid) -> bool {
        let Some(pending) = self.external_pending.remove(&id) else {
            return false;
        };
        self.finish_sync(&pending.key());
        true
    }

    fn resume_download(
        &mut self,
        pending: PendingDownload,
        requester: Requester,
        ctx: &mut ModelContext<Self>,
    ) {
        let request = DownloadRequest {
            allow_overwrite_local_changes: true,
            ..pending.request
        };
        announce(
            &requester,
            format!("Downloading {}…", request.remote_path),
            ctx,
        );
        self.spawn_download(pending.shell, request, requester, ctx);
    }

    fn resume_upload(
        &mut self,
        pending: PendingUpload,
        requester: Requester,
        ctx: &mut ModelContext<Self>,
    ) {
        let PendingUpload {
            shell,
            prepared,
            key,
            ..
        } = pending;
        announce(
            &requester,
            format!("Uploading {}…", prepared.remote_path),
            ctx,
        );

        ctx.spawn(
            async move {
                let outcome = execute_upload(shell.as_ref(), &prepared).await;
                (
                    shell,
                    prepared.mirror_root,
                    prepared.host_key,
                    prepared.remote_path,
                    outcome,
                )
            },
            move |me, (shell, mirror_root, host_key, remote_path, outcome), ctx| {
                me.finish_sync(&key);
                let finished = match outcome {
                    Ok(outcome) => {
                        link_host_mirror(shell.as_ref(), mirror_root, host_key, ctx);
                        Finished::Uploaded {
                            remote_path,
                            outcome,
                        }
                    }
                    Err(error) => Finished::Failed(error),
                };
                report(requester, finished, ctx);
            },
        );
    }

    fn spawn_download(
        &mut self,
        shell: Arc<dyn RemoteShell>,
        request: DownloadRequest,
        requester: Requester,
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
                    if let Some(host_key) =
                        outcome.host_dir.file_name().and_then(|name| name.to_str())
                    {
                        link_host_mirror(
                            shell.as_ref(),
                            request.mirror_root.clone(),
                            host_key.to_owned(),
                            ctx,
                        );
                    }
                    let finished = Finished::Downloaded {
                        remote_path: request.remote_path,
                        outcome,
                    };
                    report(requester, finished, ctx);
                }
                Ok(DownloadResult::NeedsConfirmation { modified_files }) => {
                    me.await_download_confirmation(shell, request, modified_files, requester, ctx);
                }
                Err(error) => {
                    me.finish_sync(&key);
                    report(requester, Finished::Failed(error), ctx);
                }
            },
        );
    }

    fn await_download_confirmation(
        &mut self,
        shell: Arc<dyn RemoteShell>,
        request: DownloadRequest,
        modified_files: Vec<String>,
        requester: Requester,
        ctx: &mut ModelContext<Self>,
    ) {
        let pending = PendingDownload {
            shell,
            request,
            window_id: requester.window_id(),
            edit: requester.edit_step(),
        };
        match requester {
            Requester::Window(window_id) | Requester::RemoteEdit { window_id, .. } => {
                let id = self.next_pending_id();
                self.pending_downloads.insert(id, pending);
                ctx.emit(WarpSyncEvent::DownloadNeedsConfirmation {
                    window_id,
                    id,
                    files: modified_files,
                });
            }
            Requester::External { reply, .. } => {
                let held = self
                    .hold_external(ExternalPending::Download(pending), ctx)
                    .map(|pending_id| SyncReply::NeedsConfirmation {
                        pending_id,
                        kind: ConfirmationKind::overwrite_local_changes(modified_files),
                    });
                reply.send(held);
            }
        }
    }

    fn await_upload_confirmation(
        &mut self,
        shell: Arc<dyn RemoteShell>,
        hostname: String,
        prepared: PreparedUpload,
        key: SyncKey,
        requester: Requester,
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
            new_modes: prepared.archive.new_modes.clone(),
            risks: assess(&prepared.archive.new_files, &prepared.archive.new_modes),
            creates_under: match &prepared.placement {
                UploadPlacement::Replace => None,
                UploadPlacement::Create { anchor } => Some(anchor.clone()),
            },
            missing_locally: prepared.archive.missing_locally.clone(),
            remote_check: prepared.remote_check.clone(),
            ownership_may_be_incomplete: ExtractMode::for_probe(&prepared.probe)
                == ExtractMode::Generic,
            server_id_tail: prepared
                .probe
                .machine_id
                .as_deref()
                .map(|id| id[id.len().saturating_sub(SERVER_ID_TAIL_LEN)..].to_owned()),
            diff: prepared.diff.clone(),
        };
        let pending = PendingUpload {
            shell,
            prepared,
            key,
            window_id: requester.window_id(),
            edit: requester.edit_step(),
        };
        match requester {
            Requester::Window(window_id) | Requester::RemoteEdit { window_id, .. } => {
                let id = self.next_pending_id();
                self.pending_uploads.insert(id, pending);
                ctx.emit(WarpSyncEvent::UploadNeedsConfirmation {
                    window_id,
                    id,
                    summary: Box::new(summary),
                });
            }
            Requester::External { reply, .. } => {
                let held = self
                    .hold_external(ExternalPending::Upload(Box::new(pending)), ctx)
                    .map(|pending_id| SyncReply::NeedsConfirmation {
                        pending_id,
                        kind: ConfirmationKind::upload(summary),
                    });
                reply.send(held);
            }
        }
    }
}

/// Tells the user, in the window whose shell is about to run commands, that an operation started.
fn announce(requester: &Requester, description: String, ctx: &mut ModelContext<WarpSyncModel>) {
    let description = match requester {
        Requester::Window(_) | Requester::RemoteEdit { .. } => description,
        Requester::External { .. } => format!("{EXTERNAL_TOAST_PREFIX}{description}"),
    };
    ctx.emit(WarpSyncEvent::Started {
        window_id: requester.window_id(),
        description,
    });
}

/// Delivers how an operation ended to whoever asked for it.
fn report(requester: Requester, finished: Finished, ctx: &mut ModelContext<WarpSyncModel>) {
    match requester {
        Requester::Window(window_id) => ctx.emit(window_event(window_id, finished)),
        Requester::RemoteEdit { window_id, step } => {
            let outcome = remote_edit_outcome(&finished);
            ctx.emit(window_event(window_id, finished));
            if let Some(outcome) = outcome {
                ctx.emit(WarpSyncEvent::RemoteEditFinished {
                    window_id,
                    step,
                    outcome,
                });
            }
        }
        Requester::External { reply, .. } => reply.send(finished.into_reply()),
    }
}

/// Comparisons are never part of editing a file, so they have no outcome here.
fn remote_edit_outcome(finished: &Finished) -> Option<RemoteEditOutcome> {
    match finished {
        Finished::Downloaded {
            remote_path,
            outcome,
        } => Some(RemoteEditOutcome::Downloaded {
            local_path: outcome.local_path.clone(),
            remote_path: remote_path.clone(),
            remote_user: outcome.remote_user.clone(),
        }),
        Finished::Uploaded { .. } => Some(RemoteEditOutcome::Uploaded),
        Finished::Failed(error) => Some(RemoteEditOutcome::Failed(error.to_string())),
        Finished::Compared { .. } => None,
    }
}

fn window_event(window_id: WindowId, finished: Finished) -> WarpSyncEvent {
    match finished {
        Finished::Downloaded {
            remote_path,
            outcome,
        } => WarpSyncEvent::Succeeded {
            window_id,
            message: download_message(&remote_path, &outcome),
            location: Some(MirrorLocation {
                host_dir: outcome.host_dir,
                local_path: outcome.local_path,
                is_file: outcome.is_file,
            }),
        },
        Finished::Uploaded {
            remote_path,
            outcome,
        } => WarpSyncEvent::Succeeded {
            window_id,
            message: upload_message(&remote_path, &outcome),
            location: None,
        },
        Finished::Compared {
            hostname,
            remote_path,
            outcome,
        } => compare_event(window_id, hostname, remote_path, outcome),
        Finished::Failed(error) => WarpSyncEvent::Failed { window_id, error },
    }
}

/// Tells the server directory which mirror a sync just used, when the session was opened with an
/// alias that the directory knows.
fn link_host_mirror(
    shell: &dyn RemoteShell,
    mirror_root: PathBuf,
    host_key: String,
    ctx: &mut ModelContext<WarpSyncModel>,
) {
    if !host_directory::is_enabled() {
        return;
    }
    let Some(alias) = shell
        .ssh_host()
        .and_then(|ssh_host| HostDirectoryModel::as_ref(ctx).alias_for_ssh_host(&ssh_host))
    else {
        return;
    };
    ctx.spawn(
        async move { read_mirror_link(&mirror_root, host_key) },
        move |_, observed, ctx| {
            let Some(observed) = observed else {
                return;
            };
            HostDirectoryModel::handle(ctx).update(ctx, |directory, ctx| {
                directory.observe_mirror(alias, observed, ctx)
            });
        },
    );
}

fn read_mirror_link(mirror_root: &Path, host_key: String) -> Option<MirrorLink> {
    match Manifest::load_or_default(&manifest_path(mirror_root, &host_key), &host_key) {
        Ok(manifest) => Some(MirrorLink {
            machine_id: manifest.machine_id().map(str::to_owned),
            mirror_key: host_key,
        }),
        Err(error) => {
            log::warn!("Warp Sync: could not read the manifest of {host_key}: {error}");
            None
        }
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

fn compare_event(
    window_id: WindowId,
    hostname: String,
    remote_path: String,
    outcome: CompareOutcome,
) -> WarpSyncEvent {
    let Some(diff_path) = outcome.diff_path else {
        return WarpSyncEvent::Succeeded {
            window_id,
            message: format!(
                "No differences: {remote_path} matches the local mirror ({})",
                pluralize_count(outcome.identical_files, "file")
            ),
            location: None,
        };
    };
    WarpSyncEvent::CompareFinished {
        window_id,
        summary: Box::new(CompareSummary {
            remote_user: outcome.remote_user,
            hostname,
            remote_path,
            differences: outcome.differences,
            identical_files: outcome.identical_files,
            diff_path,
            host_dir: outcome.host_dir,
            server_copy_dir: outcome.server_copy_dir,
        }),
    }
}

fn download_message(remote_path: &str, outcome: &DownloadOutcome) -> String {
    let mut message = format!(
        "Downloaded {} and {} ({}) from {remote_path} as {}",
        pluralize_count(outcome.files, "file"),
        pluralize_count(outcome.dirs, "folder"),
        format_size(outcome.total_bytes),
        outcome.remote_user
    );
    if !outcome.skipped.is_empty() {
        message.push_str(&format!(
            ". Skipped {} link(s), special file(s) or .git folder(s)",
            outcome.skipped.len()
        ));
    }
    append_baseline_warning(&mut message, outcome.baseline_warning.as_deref());
    message
}

fn upload_message(remote_path: &str, outcome: &UploadOutcome) -> String {
    let mut message = format!(
        "Uploaded {} and {} ({}) to {remote_path} as {}",
        pluralize_count(outcome.files, "file"),
        pluralize_count(outcome.dirs, "folder"),
        format_size(outcome.content_bytes),
        outcome.remote_user
    );
    if let Some(backup_path) = &outcome.backup_path {
        message.push_str(&format!(". Previous version saved to {backup_path}"));
    }
    append_baseline_warning(&mut message, outcome.baseline_warning.as_deref());
    message
}

fn append_baseline_warning(message: &mut String, warning: Option<&str>) {
    if let Some(warning) = warning {
        message.push_str(&format!(". {warning}"));
    }
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
