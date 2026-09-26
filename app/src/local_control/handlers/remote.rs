//! `remote.*` actions: lets a local-control client run commands and read and write files in a
//! remote session that the user attached, with the privileges of that session's shell.
//!
//! The client has to name the session explicitly, so that a request can never land in whichever
//! pane happens to be focused. What may be done in it is decided by the user's attachment, not by
//! the client.

use std::sync::Arc;

use ::local_control::protocol::{
    Action, RemoteAccess, RemoteAttachment, RemoteExecParams, RemoteFileReadParams,
    RemoteFileWriteParams, RemoteOutputRecentParams, RemoteSessionKind, RemoteSessionListResult,
    RemoteSessionRef, RemoteSessionSummary, RequestEnvelope, SessionTarget, TargetSelector,
};
use ::local_control::{ActionKind, ControlError, ErrorCode};
use futures::channel::oneshot;
use parking_lot::FairMutex;
use serde_json::Value;
use warpui::{ModelContext, SingletonEntity};

use super::metadata::{SessionEntry, session_entries};
use crate::agent_bridge::attachments::{Access, AttachmentStatus};
use crate::agent_bridge::audit::audit_dir;
use crate::agent_bridge::error::AgentBridgeError;
use crate::agent_bridge::model::AgentBridgeModel;
use crate::agent_bridge::ops::{self, SessionRunner, Target};
use crate::agent_bridge::recent;
use crate::features::FeatureFlag;
use crate::local_control::LocalControlBridge;
use crate::terminal::model::TerminalModel;
use crate::terminal::model::session::Session;
use crate::warp_sync::printable;

pub(crate) type RemoteReceiver = oneshot::Receiver<Result<Value, ControlError>>;

/// A terminal session as seen from the main thread.
struct SessionSnapshot {
    /// The id clients use to address the session.
    session_id: String,
    session: Arc<Session>,
    cwd: Option<String>,
    terminal_model: Arc<FairMutex<TerminalModel>>,
}

/// One of the operations that run on the server.
enum Operation {
    Exec(RemoteExecParams),
    Read(RemoteFileReadParams),
    Write(RemoteFileWriteParams),
}

impl Operation {
    fn parse(action: &Action) -> Result<Self, ControlError> {
        match action.kind {
            ActionKind::RemoteExec => action.params_as().map(Self::Exec),
            ActionKind::RemoteFileRead => action.params_as().map(Self::Read),
            ActionKind::RemoteFileWrite => action.params_as().map(Self::Write),
            kind => Err(ControlError::new(
                ErrorCode::UnsupportedAction,
                format!("{} does not run on a server", kind.as_str()),
            )),
        }
    }

    /// The access the user must have granted for this operation.
    fn needed_access(&self) -> Access {
        match self {
            Self::Read(_) => Access::ReadOnly,
            Self::Exec(_) | Self::Write(_) => Access::Full,
        }
    }

    fn is_exec(&self) -> bool {
        matches!(self, Self::Exec(_))
    }

    async fn run(self, runner: &SessionRunner, target: &Target) -> Result<Value, AgentBridgeError> {
        match self {
            Self::Exec(params) => ops::exec(runner, target, params).await,
            Self::Read(params) => ops::read_file(runner, target, params).await,
            Self::Write(params) => ops::write_file(runner, target, params).await,
        }
    }
}

pub(crate) fn session_list(
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<Value, ControlError> {
    let kind = ActionKind::RemoteSessionList;
    ensure_enabled(kind)?;
    let entries = session_entries(&TargetSelector::default(), kind, ctx)?;
    let sessions = entries
        .iter()
        .filter_map(|entry| {
            let snapshot = read_session(entry, ctx)?;
            Some(summary(entry, &snapshot, ctx))
        })
        .collect();
    serde_json::to_value(RemoteSessionListResult { sessions }).map_err(|err| {
        ControlError::with_details(
            ErrorCode::Internal,
            "failed to serialize the session list",
            err.to_string(),
        )
    })
}

/// Starts an operation in an attached session. The answer arrives on the returned receiver so
/// that the main thread is not held up while the server works.
pub(crate) fn start(
    request: &RequestEnvelope,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<RemoteReceiver, ControlError> {
    let kind = request.action.kind;
    ensure_enabled(kind)?;
    let snapshot = resolve(&request.target, kind, ctx)?;
    let session = snapshot.session;
    let operation = Operation::parse(&request.action)?;
    let runner = SessionRunner::new(session.clone()).map_err(ControlError::from)?;
    check_access(&session, operation.needed_access(), ctx)?;

    let id = session.id();
    let target = target(snapshot.session_id, &session, snapshot.cwd, request);
    let is_exec = operation.is_exec();
    let (sender, receiver) = oneshot::channel();
    ctx.spawn(
        async move { operation.run(&runner, &target).await },
        move |_, result, ctx| {
            AgentBridgeModel::handle(ctx).update(ctx, |model, _| model.record_use(id, is_exec));
            if sender.send(result.map_err(ControlError::from)).is_err() {
                log::debug!("A local-control client stopped waiting for a remote result");
            }
        },
    );
    Ok(receiver)
}

/// Copies the latest commands of an attached session out of its blocks. The answer arrives on the
/// returned receiver so that writing the audit log does not hold up the main thread.
pub(crate) fn output_recent(
    request: &RequestEnvelope,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<RemoteReceiver, ControlError> {
    let kind = request.action.kind;
    ensure_enabled(kind)?;
    let params = request.action.params_as::<RemoteOutputRecentParams>()?;
    let count = recent::block_count(params.count).map_err(ControlError::from)?;
    ops::validate_agent(params.agent.as_deref()).map_err(ControlError::from)?;
    let snapshot = resolve(&request.target, kind, ctx)?;
    let session = snapshot.session;
    ops::ensure_supported(&session).map_err(ControlError::from)?;
    check_access(&session, Access::ReadOnly, ctx)?;

    let id = session.id();
    let blocks = {
        let model = snapshot.terminal_model.lock();
        recent::capture(model.block_list(), id, count)
    };
    let target = target(snapshot.session_id, &session, snapshot.cwd, request);
    let (sender, receiver) = oneshot::channel();
    ctx.spawn(
        async move { ops::recent_output(&target, params.agent.as_deref(), blocks) },
        move |_, result, ctx| {
            AgentBridgeModel::handle(ctx).update(ctx, |model, _| model.record_use(id, false));
            if sender.send(result.map_err(ControlError::from)).is_err() {
                log::debug!("A local-control client stopped waiting for a remote result");
            }
        },
    );
    Ok(receiver)
}

/// Fails unless the user attached `session` with at least `needed` access.
fn check_access(
    session: &Session,
    needed: Access,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<(), ControlError> {
    let id = session.id();
    AgentBridgeModel::handle(ctx)
        .update(ctx, |model, _| {
            model.check(id, needed, session.user(), session.hostname())
        })
        .map_err(ControlError::from)
}

fn target(
    session_id: String,
    session: &Session,
    cwd: Option<String>,
    request: &RequestEnvelope,
) -> Target {
    Target {
        session: RemoteSessionRef {
            session_id,
            host: printable(session.hostname()),
            user: printable(session.user()),
        },
        cwd,
        request_id: request.request_id,
        audit_dir: audit_dir(),
    }
}

fn ensure_enabled(kind: ActionKind) -> Result<(), ControlError> {
    if FeatureFlag::AgentBridge.is_enabled() {
        return Ok(());
    }
    Err(ControlError::new(
        ErrorCode::UnsupportedAction,
        format!(
            "{} requires the Agent Bridge, which is not enabled in this build",
            kind.as_str()
        ),
    ))
}

/// The session the request names. Only an explicit id is accepted.
fn resolve(
    target: &TargetSelector,
    kind: ActionKind,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<SessionSnapshot, ControlError> {
    match &target.session {
        Some(SessionTarget::Id { .. }) => {}
        Some(SessionTarget::Active) | None => {
            return Err(ControlError::new(
                ErrorCode::MissingTarget,
                format!(
                    "{} needs the id of a session; list them with remote.session.list",
                    kind.as_str()
                ),
            ));
        }
    }
    let stale = || {
        ControlError::new(
            ErrorCode::StaleTarget,
            format!("{} cannot find that session any more", kind.as_str()),
        )
    };
    let entry = session_entries(target, kind, ctx)?
        .into_iter()
        .next()
        .ok_or_else(stale)?;
    read_session(&entry, ctx).ok_or_else(stale)
}

fn read_session(
    entry: &SessionEntry,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Option<SessionSnapshot> {
    entry.pane_group.read(ctx, |pane_group, ctx| {
        let terminal = pane_group.terminal_view_from_pane_id(entry.pane_id, ctx)?;
        terminal.read(ctx, |view, ctx| {
            let active = view.active_session().as_ref(ctx);
            Some(SessionSnapshot {
                session_id: entry.pane_id.to_string(),
                session: active.session(ctx)?,
                cwd: active.current_working_directory().cloned(),
                terminal_model: view.model.clone(),
            })
        })
    })
}

fn summary(
    entry: &SessionEntry,
    snapshot: &SessionSnapshot,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> RemoteSessionSummary {
    let session = &snapshot.session;
    let id = session.id();
    let attached = AgentBridgeModel::handle(ctx).read(ctx, |model, _| model.status(id));
    RemoteSessionSummary {
        session_id: snapshot.session_id.clone(),
        window_index: entry.window_index as u32,
        tab_index: entry.tab_index as u32,
        pane_index: entry.pane_index as u32,
        is_active: entry.is_active,
        session_type: if session.is_local() {
            RemoteSessionKind::Local
        } else {
            RemoteSessionKind::Remote
        },
        host: printable(session.hostname()),
        user: printable(session.user()),
        shell: session.shell().shell_type().name().to_owned(),
        cwd: snapshot.cwd.as_deref().map(printable),
        attached: attached.map(remote_attachment),
    }
}

fn remote_attachment(status: AttachmentStatus) -> RemoteAttachment {
    RemoteAttachment {
        access: match status.access {
            Access::ReadOnly => RemoteAccess::ReadOnly,
            Access::Full => RemoteAccess::Full,
        },
        idle_secs: status.idle.as_secs(),
        expires_in_secs: status.expires_in.as_secs(),
        exec_count: status.exec_count,
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
