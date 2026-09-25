//! `sync.*` actions: lets a local-control client drive Warp Sync through a session that is open
//! in Warp.
//!
//! The client only names a path in the local mirror. The host and the remote path are worked out
//! from that path, and the session that runs the commands is chosen among the open remote
//! sessions of the same host, so a mirror is never used through another machine's session.

use std::sync::Arc;

use ::local_control::protocol::{
    Action, SyncPathParams, SyncPathStatus, SyncPendingParams, SyncResult, SyncSessionSummary,
    SyncStatusParams, TargetSelector,
};
use ::local_control::{ActionKind, ControlError, ErrorCode};
use futures::channel::oneshot;
use warpui::{ModelContext, SingletonEntity};

use super::metadata::session_entries;
use super::sync_reply::{control_error, sync_result};
use crate::features::FeatureFlag;
use crate::local_control::LocalControlBridge;
use crate::terminal::model::session::Session;
use crate::warp_sync::external::{
    MirrorPath, SessionCandidate, choose_session, resolve_mirror_path,
};
use crate::warp_sync::{
    ExternalReply, Requester, SyncConfig, SyncReply, WarpSyncError, WarpSyncModel,
    host_dir_matches, printable,
};

pub(crate) type SyncReceiver = oneshot::Receiver<Result<serde_json::Value, ControlError>>;
type SyncSender = oneshot::Sender<Result<serde_json::Value, ControlError>>;

/// An operation on one path of the mirror.
#[derive(Clone, Copy)]
pub(crate) enum PathOperation {
    Download,
    UploadPrepare,
    Compare,
}

pub(crate) fn status(
    action: &Action,
    target: &TargetSelector,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<SyncReceiver, ControlError> {
    ensure_enabled(action.kind)?;
    let params = action.params_as::<SyncStatusParams>()?;
    let config = SyncConfig::from_settings(ctx).map_err(control_error)?;
    let mirror_root = config.mirror_root.to_string_lossy().into_owned();
    let (sender, receiver) = oneshot::channel();
    let Some(path) = params.path else {
        finish(
            sender,
            Ok(SyncResult::Status {
                mirror_root,
                path: None,
            }),
        );
        return Ok(receiver);
    };

    let target = target.clone();
    ctx.spawn(
        async move { resolve_mirror_path(&config.mirror_root, &path) },
        move |_, resolved, ctx| {
            let result = resolved.map_err(control_error).and_then(|mirror| {
                let sessions = remote_sessions(&target, ActionKind::SyncStatus, ctx)?;
                Ok(SyncResult::Status {
                    mirror_root,
                    path: Some(path_status(mirror, sessions)),
                })
            });
            finish(sender, result);
        },
    );
    Ok(receiver)
}

pub(crate) fn path_operation(
    operation: PathOperation,
    action: &Action,
    target: &TargetSelector,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<SyncReceiver, ControlError> {
    ensure_enabled(action.kind)?;
    let params = action.params_as::<SyncPathParams>()?;
    let config = SyncConfig::from_settings(ctx).map_err(control_error)?;
    let (sender, receiver) = oneshot::channel();
    let target = target.clone();
    let kind = action.kind;
    ctx.spawn(
        async move { resolve_mirror_path(&config.mirror_root, &params.path) },
        move |_, resolved, ctx| match resolved {
            Ok(mirror) => start_operation(operation, kind, mirror, &target, sender, ctx),
            Err(error) => finish(sender, Err(control_error(error))),
        },
    );
    Ok(receiver)
}

pub(crate) fn confirm(
    action: &Action,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<SyncReceiver, ControlError> {
    ensure_enabled(action.kind)?;
    let params = action.params_as::<SyncPendingParams>()?;
    let (sender, receiver) = oneshot::channel();
    let (reply, replies) = ExternalReply::channel();
    WarpSyncModel::handle(ctx).update(ctx, |model, ctx| {
        model.confirm_external(params.pending_id, reply, ctx)
    });
    forward(replies, sender, ctx);
    Ok(receiver)
}

pub(crate) fn cancel(
    action: &Action,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<SyncReceiver, ControlError> {
    ensure_enabled(action.kind)?;
    let params = action.params_as::<SyncPendingParams>()?;
    let (sender, receiver) = oneshot::channel();
    let cancelled =
        WarpSyncModel::handle(ctx).update(ctx, |model, _| model.cancel_external(params.pending_id));
    finish(
        sender,
        cancelled
            .map(|()| SyncResult::Cancelled)
            .map_err(control_error),
    );
    Ok(receiver)
}

fn ensure_enabled(kind: ActionKind) -> Result<(), ControlError> {
    if FeatureFlag::WarpSync.is_enabled() {
        return Ok(());
    }
    Err(ControlError::new(
        ErrorCode::UnsupportedAction,
        format!(
            "{} requires Warp Sync, which is not enabled in this build",
            kind.as_str()
        ),
    ))
}

/// Hands the operation to Warp Sync through the session that belongs to the mirror's host.
fn start_operation(
    operation: PathOperation,
    kind: ActionKind,
    mirror: MirrorPath,
    target: &TargetSelector,
    sender: SyncSender,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    let MirrorPath {
        host_dir_name,
        remote_path,
    } = mirror;
    let chosen = remote_path
        .ok_or_else(|| {
            control_error(WarpSyncError::InvalidPath(
                "choose a file or folder inside the host folder, not the host folder itself"
                    .to_owned(),
            ))
        })
        .and_then(|remote_path| {
            let sessions = remote_sessions(target, kind, ctx)?;
            let focused_window = ctx.windows().active_window();
            let chosen =
                choose_session(sessions, &host_dir_name, focused_window).map_err(control_error)?;
            Ok((remote_path, chosen))
        });
    let (remote_path, chosen) = match chosen {
        Ok(chosen) => chosen,
        Err(error) => return finish(sender, Err(error)),
    };

    let (requester, replies) = Requester::external(chosen.window_id, Some(host_dir_name));
    let session = chosen.session;
    WarpSyncModel::handle(ctx).update(ctx, |model, ctx| match operation {
        PathOperation::Download => model.start_download(session, remote_path, requester, ctx),
        PathOperation::UploadPrepare => model.start_upload(session, remote_path, requester, ctx),
        PathOperation::Compare => model.start_compare(session, remote_path, requester, ctx),
    });
    forward(replies, sender, ctx);
}

/// Waits for Warp Sync's answer without holding up the main thread, then hands it to the client.
/// A confirmation that nobody is left to answer is dropped at once instead of holding its path
/// until it expires.
fn forward(
    replies: oneshot::Receiver<Result<SyncReply, WarpSyncError>>,
    sender: SyncSender,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    ctx.spawn(replies, move |_, reply, ctx| {
        let unanswered = match &reply {
            Ok(Ok(SyncReply::NeedsConfirmation { pending_id, .. })) => Some(*pending_id),
            Ok(Ok(_) | Err(_)) | Err(_) => None,
        };
        let result = match reply {
            Ok(Ok(reply)) => Ok(sync_result(reply)),
            Ok(Err(error)) => Err(control_error(error)),
            Err(oneshot::Canceled) => Err(ControlError::new(
                ErrorCode::Internal,
                "Warp Sync stopped before it answered",
            )),
        };
        let delivered = deliver(sender, result);
        if let (false, Some(pending_id)) = (delivered, unanswered) {
            let dropped = WarpSyncModel::handle(ctx)
                .update(ctx, |model, _| model.cancel_external(pending_id));
            if let Err(error) = dropped {
                log::debug!("A dropped Warp Sync confirmation was already gone: {error}");
            }
        }
    });
}

/// Sends `result` to a client that may have stopped waiting.
fn finish(sender: SyncSender, result: Result<SyncResult, ControlError>) {
    deliver(sender, result);
}

/// Sends `result` to the client. Returns whether the client was still waiting.
fn deliver(sender: SyncSender, result: Result<SyncResult, ControlError>) -> bool {
    let value = result.and_then(|result| {
        serde_json::to_value(result).map_err(|err| {
            ControlError::with_details(
                ErrorCode::Internal,
                "failed to serialize the Warp Sync result",
                err.to_string(),
            )
        })
    });
    let delivered = sender.send(value).is_ok();
    if !delivered {
        log::debug!("A local-control client stopped waiting for a Warp Sync result");
    }
    delivered
}

/// The remote sessions that `target` selects, or all of them when it selects none.
fn remote_sessions(
    target: &TargetSelector,
    kind: ActionKind,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<Vec<SessionCandidate<Arc<Session>>>, ControlError> {
    let entries = session_entries(target, kind, ctx)?;
    let mut candidates = Vec::new();
    for entry in entries {
        let session = entry.pane_group.read(ctx, |pane_group, ctx| {
            pane_group
                .terminal_view_from_pane_id(entry.pane_id, ctx)
                .and_then(|terminal| {
                    terminal.read(ctx, |view, ctx| {
                        view.active_session().as_ref(ctx).session(ctx)
                    })
                })
        });
        let Some(session) = session.filter(|session| !session.is_local()) else {
            continue;
        };
        candidates.push(SessionCandidate {
            hostname: session.hostname().to_owned(),
            user: session.user().to_owned(),
            session,
            session_id: entry.pane_id.to_string(),
            window_id: entry.window_id,
            tab_index: entry.tab_index as u32,
            is_active: entry.is_active,
        });
    }
    Ok(candidates)
}

fn path_status(
    mirror: MirrorPath,
    sessions: Vec<SessionCandidate<Arc<Session>>>,
) -> SyncPathStatus {
    let sessions = sessions
        .into_iter()
        .filter(|candidate| host_dir_matches(&mirror.host_dir_name, &candidate.hostname))
        .map(|candidate| SyncSessionSummary {
            session_id: candidate.session_id,
            window_id: candidate.window_id.to_string(),
            tab_index: candidate.tab_index,
            hostname: printable(&candidate.hostname),
            user: printable(&candidate.user),
            is_active: candidate.is_active,
        })
        .collect();
    SyncPathStatus {
        host_key: mirror.host_dir_name,
        remote_path: mirror.remote_path,
        sessions,
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
