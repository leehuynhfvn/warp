//! `remote.*` actions: lets a local-control client run commands and read and write files in a
//! remote session that the user attached, with the privileges of that session's shell.
//!
//! The client has to name the session explicitly, so that a request can never land in whichever
//! pane happens to be focused. What may be done in it is decided by the user's attachment, not by
//! the client.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::local_control::protocol::{
    APPROVAL_TIMEOUT_SECS, Action, RemoteAccess, RemoteAttachment, RemoteExecParams,
    RemoteExecVisibleParams, RemoteFileReadParams, RemoteFileWriteParams, RemoteOutputRecentParams,
    RemoteSessionKind, RemoteSessionListResult, RemoteSessionRef, RemoteSessionSummary,
    RequestEnvelope, SessionTarget, TargetSelector, WriteExpectation,
};
use ::local_control::{ActionKind, ControlError, ErrorCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::channel::oneshot;
use parking_lot::FairMutex;
use serde_json::Value;
use warpui::{ModelContext, SingletonEntity, ViewHandle, WindowId};

use super::metadata::{SessionEntry, session_entries};
use crate::agent_bridge::approval::{
    self, AgentLabel, ApprovalDecision, ApprovalRequest, ApprovalSubject,
};
use crate::agent_bridge::attachments::{Access, AttachmentStatus};
use crate::agent_bridge::audit::audit_dir;
use crate::agent_bridge::error::AgentBridgeError;
use crate::agent_bridge::model::AgentBridgeModel;
use crate::agent_bridge::operations::OperationKind;
use crate::agent_bridge::ops::{self, SessionRunner, Target, VisibleAudit};
use crate::agent_bridge::pairing;
use crate::agent_bridge::policy::{self, Decision, PolicyRequest};
use crate::agent_bridge::visible::{self, CommandWatch};
use crate::agent_bridge::{APPROVAL_PREVIEW_LINES, recent};
use crate::features::FeatureFlag;
use crate::local_control::LocalControlBridge;
use crate::terminal::model::TerminalModel;
use crate::terminal::model::session::{Session, SessionId};
use crate::terminal::view::TerminalView;
use crate::warp_sync::printable;

pub(crate) type RemoteReceiver = oneshot::Receiver<Result<Value, ControlError>>;

/// A terminal session as seen from the main thread.
struct SessionSnapshot {
    /// The id clients use to address the session.
    session_id: String,
    session: Arc<Session>,
    cwd: Option<String>,
    terminal_model: Arc<FairMutex<TerminalModel>>,
    terminal_view: ViewHandle<TerminalView>,
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

    fn agent(&self) -> Option<&str> {
        match self {
            Self::Exec(params) => params.agent.as_deref(),
            Self::Read(params) => params.agent.as_deref(),
            Self::Write(params) => params.agent.as_deref(),
        }
    }

    /// What the agent-ops policy evaluates for this operation; `None` for a read, which the
    /// policy does not gate (mục 3.1 of the O2 plan: L0 actions run unchanged from O1).
    fn policy_subject(&self) -> Option<PolicySubject> {
        match self {
            Self::Exec(params) => Some(PolicySubject::Exec(params.command.clone())),
            Self::Write(params) => Some(PolicySubject::Write {
                path: params.path.clone(),
                content_base64: params.content_base64.clone(),
                creates: matches!(params.expectation, WriteExpectation::MustNotExist),
            }),
            Self::Read(_) => None,
        }
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
/// that the main thread is not held up while the server works. A write (`Exec`/`Write`) is gated
/// on the agent-ops policy first; a read runs unchanged from O1. `token_sha256` is the hash of the
/// envelope's `agent_token`, already computed once by the caller (mục 3.11 of the O2 plan).
pub(crate) fn start(
    request: &RequestEnvelope,
    token_sha256: Option<String>,
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
    let window_id = snapshot.terminal_view.window_id(ctx);
    let target = target(snapshot.session_id, &session, snapshot.cwd, request);
    let is_exec = operation.is_exec();
    let (sender, receiver) = oneshot::channel();

    let Some(subject) = operation.policy_subject() else {
        run_hidden_operation(id, operation, runner, target, is_exec, sender, ctx);
        return Ok(receiver);
    };
    let input = AuthorizeInput {
        hostname: session.hostname().to_owned(),
        session: id,
        window_id,
        subject,
        action: kind,
        agent: AgentLabel {
            claimed: operation.agent().map(str::to_owned),
            agent_id: None,
        },
        token_sha256,
        target: target.clone(),
    };
    authorize(
        input,
        move |result, ctx| match result {
            Ok(outcome) => {
                let target = Target {
                    policy_decision: outcome.policy_decision,
                    agent_id: outcome.agent_id,
                    ..target
                };
                run_hidden_operation(id, operation, runner, target, is_exec, sender, ctx);
            }
            Err(error) => {
                if sender.send(Err(error.into())).is_err() {
                    log::debug!("A local-control client stopped waiting for a remote result");
                }
            }
        },
        ctx,
    );
    Ok(receiver)
}

/// Takes the session's Hidden operation slot and runs `operation`, delivering every outcome
/// (including a failure to take the slot) through `sender`. Split out of `start` so the slot is
/// only held for the run itself, never for an agent-ops approval wait (mục 2.1 P3 of the plan).
fn run_hidden_operation(
    id: SessionId,
    operation: Operation,
    runner: SessionRunner,
    target: Target,
    is_exec: bool,
    sender: oneshot::Sender<Result<Value, ControlError>>,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    let begun = AgentBridgeModel::handle(ctx)
        .update(ctx, |model, _| {
            model.begin_operation(id, OperationKind::Hidden)
        })
        .map_err(ControlError::from);
    if let Err(error) = begun {
        if sender.send(Err(error)).is_err() {
            log::debug!("A local-control client stopped waiting for a remote result");
        }
        return;
    }

    ctx.spawn(
        async move { operation.run(&runner, &target).await },
        move |_, result, ctx| {
            AgentBridgeModel::handle(ctx).update(ctx, |model, _| {
                model.end_operation(id, OperationKind::Hidden);
                model.record_use(id, is_exec);
            });
            if sender.send(result.map_err(ControlError::from)).is_err() {
                log::debug!("A local-control client stopped waiting for a remote result");
            }
        },
    );
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

/// Types a command into an attached session's shell, where it runs as a block the user watches.
/// The answer is read from that block once the command finishes or the wait times out. Gated on
/// the agent-ops policy first: the command is only ever typed into the shell after it is allowed
/// (mục 2.4 P14 of the plan).
pub(crate) fn exec_visible(
    request: &RequestEnvelope,
    token_sha256: Option<String>,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<RemoteReceiver, ControlError> {
    let kind = request.action.kind;
    ensure_enabled(kind)?;
    let params = request.action.params_as::<RemoteExecVisibleParams>()?;
    visible::validate(&params).map_err(ControlError::from)?;
    let snapshot = resolve(&request.target, kind, ctx)?;
    let session = snapshot.session.clone();
    ops::ensure_supported(&session).map_err(ControlError::from)?;
    check_access(&session, Access::Full, ctx)?;
    let id = session.id();
    let window_id = snapshot.terminal_view.window_id(ctx);
    let target = target(
        snapshot.session_id.clone(),
        &session,
        snapshot.cwd.clone(),
        request,
    );
    let (sender, receiver) = oneshot::channel();

    let input = AuthorizeInput {
        hostname: session.hostname().to_owned(),
        session: id,
        window_id,
        subject: PolicySubject::ExecVisible(params.command.clone()),
        action: kind,
        agent: AgentLabel {
            claimed: params.agent.clone(),
            agent_id: None,
        },
        token_sha256,
        target: target.clone(),
    };
    authorize(
        input,
        move |result, ctx| match result {
            Ok(outcome) => {
                let target = Target {
                    policy_decision: outcome.policy_decision,
                    agent_id: outcome.agent_id,
                    ..target
                };
                run_visible_operation(id, snapshot, params, target, sender, ctx);
            }
            Err(error) => {
                if sender.send(Err(error.into())).is_err() {
                    log::debug!("A local-control client stopped waiting for a remote result");
                }
            }
        },
        ctx,
    );
    Ok(receiver)
}

/// Takes the session's Visible operation slot and types `params.command` into its shell,
/// delivering every outcome through `sender`. Split out of `exec_visible` so the slot, and the
/// shell itself, are only touched once the request is allowed (mục 2.1 P3/P14 of the plan).
fn run_visible_operation(
    id: SessionId,
    snapshot: SessionSnapshot,
    params: RemoteExecVisibleParams,
    target: Target,
    sender: oneshot::Sender<Result<Value, ControlError>>,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    let begun = AgentBridgeModel::handle(ctx)
        .update(ctx, |model, _| {
            model.begin_operation(id, OperationKind::Visible)
        })
        .map_err(ControlError::from);
    if let Err(error) = begun {
        if sender.send(Err(error)).is_err() {
            log::debug!("A local-control client stopped waiting for a remote result");
        }
        return;
    }

    let mut audit = VisibleAudit::begin(&target, &params);
    ctx.spawn(
        async move { audit.record_start().map(|()| audit) },
        move |_, started, ctx| {
            let audit = match started {
                Ok(audit) => audit,
                Err(error) => return finish_visible(id, Err(error), sender, ctx),
            };
            let watch = match send_visible_command(&snapshot, id, &params.command, ctx) {
                Ok(watch) => watch,
                Err(error) => {
                    ctx.spawn(
                        async move {
                            audit.finish(Err(&error));
                            error
                        },
                        move |_, error, ctx| finish_visible(id, Err(error), sender, ctx),
                    );
                    return;
                }
            };
            let session = target.session;
            let timeout = visible::timeout(&params);
            ctx.spawn(
                async move {
                    let result = visible::wait(snapshot.terminal_model, watch, timeout)
                        .await
                        .map(|outcome| visible::result(session, outcome));
                    audit.finish(result.as_ref());
                    result
                },
                move |_, result, ctx| {
                    let value = result.and_then(|result| {
                        serde_json::to_value(result).map_err(|err| {
                            AgentBridgeError::Io(format!("could not encode the result: {err}"))
                        })
                    });
                    finish_visible(id, value, sender, ctx);
                },
            );
        },
    );
}

/// Types `command` into the session's shell unless the shell is busy or is no longer the one the
/// user attached. The terminal model is released before the input is touched, which locks it
/// again.
fn send_visible_command(
    snapshot: &SessionSnapshot,
    id: SessionId,
    command: &str,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<CommandWatch, AgentBridgeError> {
    let active = snapshot.terminal_view.read(ctx, |view, ctx| {
        view.active_session()
            .as_ref(ctx)
            .session(ctx)
            .map(|session| session.id())
    });
    if active != Some(id) {
        return Err(AgentBridgeError::Executor(
            "the session's shell changed before the command was sent (for example after an \
             exit from sudo -i); list the sessions again"
                .to_owned(),
        ));
    }
    let start = {
        let model = snapshot.terminal_model.lock();
        let active_block = model.block_list().active_block();
        if active_block.is_active_and_long_running() && !active_block.is_in_band_command_block() {
            return Err(AgentBridgeError::SessionBusy);
        }
        model.block_list().active_block_index()
    };
    let sent = snapshot.terminal_view.update(ctx, |view, ctx| {
        view.input().update(ctx, |input, ctx| {
            input.try_execute_command_preserving_input(command, ctx)
        })
    });
    if !sent {
        return Err(AgentBridgeError::SessionBusy);
    }
    Ok(CommandWatch::new(id, command.to_owned(), start))
}

fn finish_visible(
    id: SessionId,
    result: Result<Value, AgentBridgeError>,
    sender: oneshot::Sender<Result<Value, ControlError>>,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    AgentBridgeModel::handle(ctx).update(ctx, |model, _| {
        model.end_operation(id, OperationKind::Visible);
        model.record_use(id, true);
    });
    if sender.send(result.map_err(ControlError::from)).is_err() {
        log::debug!("A local-control client stopped waiting for a remote result");
    }
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
        policy_decision: None,
        agent_id: None,
    }
}

/// Enough about a write request to evaluate the agent-ops policy on a background thread and, if
/// it is denied or asked about, to audit or display it. Owned (not the borrowing `PolicyRequest`)
/// so it can cross into the spawned future and back.
#[derive(Debug, Clone)]
enum PolicySubject {
    Exec(String),
    ExecVisible(String),
    Write {
        path: String,
        content_base64: String,
        /// Whether the write expects to create a new file (`WriteExpectation::MustNotExist`)
        /// rather than replace an existing one.
        creates: bool,
    },
}

impl PolicySubject {
    fn as_policy_request(&self) -> PolicyRequest<'_> {
        match self {
            Self::Exec(command) => PolicyRequest::Exec(command),
            Self::ExecVisible(command) => PolicyRequest::ExecVisible(command),
            Self::Write { path, .. } => PolicyRequest::Write { path },
        }
    }

    fn command(&self) -> Option<&str> {
        match self {
            Self::Exec(command) | Self::ExecVisible(command) => Some(command),
            Self::Write { .. } => None,
        }
    }

    fn path(&self) -> Option<&str> {
        match self {
            Self::Write { path, .. } => Some(path),
            Self::Exec(_) | Self::ExecVisible(_) => None,
        }
    }

    /// Builds what an approval dialog shows for this request. `cwd` comes from `Target` rather
    /// than living on `PolicySubject` itself.
    fn into_approval_subject(self, cwd: Option<String>) -> ApprovalSubject {
        match self {
            Self::Exec(command) => ApprovalSubject::Command {
                command,
                cwd,
                visible: false,
            },
            Self::ExecVisible(command) => ApprovalSubject::Command {
                command,
                cwd,
                visible: true,
            },
            Self::Write {
                path,
                content_base64,
                creates,
            } => {
                let (bytes, preview, preview_truncated_lines) = write_preview(&content_base64);
                ApprovalSubject::Write {
                    path,
                    bytes,
                    creates,
                    preview,
                    preview_truncated_lines,
                }
            }
        }
    }
}

/// Decodes `content_base64` (a pending `remote.file.write`'s content) into what an approval
/// dialog shows: the byte count, up to `APPROVAL_PREVIEW_LINES` lines of text, and how many more
/// lines were cut. Invalid base64 previews as empty rather than failing the approval outright —
/// the write itself still validates it properly, and rejects it, once (if) it is allowed to run.
fn write_preview(content_base64: &str) -> (u64, String, usize) {
    let Ok(bytes) = BASE64.decode(content_base64.trim()) else {
        return (0, String::new(), 0);
    };
    let total = bytes.len() as u64;
    match std::str::from_utf8(&bytes) {
        Ok(text) => {
            let mut lines = text.lines();
            let preview: Vec<&str> = lines.by_ref().take(APPROVAL_PREVIEW_LINES).collect();
            let preview_truncated_lines = lines.count();
            (total, preview.join("\n"), preview_truncated_lines)
        }
        Err(_) => (total, format!("(binary, {total} bytes)"), 0),
    }
}

/// What `authorize` needs to evaluate the agent-ops policy, and if it asks about or denies the
/// request, to queue it for approval or to audit the denial. `target`/`action`/`agent` mirror what
/// the operation would itself audit as its "started" record, since a request that is never
/// allowed never reaches it.
struct AuthorizeInput {
    hostname: String,
    session: SessionId,
    /// The window whose pane holds the session, so a toast for this request shows up there.
    window_id: WindowId,
    subject: PolicySubject,
    action: ActionKind,
    /// `agent_id` starts `None` and is filled in by `authorize` once it has resolved
    /// `token_sha256` against the paired-agents store, on the same background turn that loads the
    /// policy (mục 3.11 of the plan).
    agent: AgentLabel,
    /// Hash of the envelope's `agent_token`, already computed once by the caller.
    token_sha256: Option<String>,
    target: Target,
}

/// What `authorize` decided once a request is allowed to run: the label its "started" audit record
/// gets, and the paired identity (if any) every audit record for the request should carry.
struct AuthorizedOutcome {
    policy_decision: Option<&'static str>,
    agent_id: Option<String>,
}

/// Decides whether an agent may perform `input.subject`, per the fixed rule order in mục 2.2 of
/// the O2 plan (pairing, host selection, `[deny]`, then the rule's mode), then runs
/// `continue_with` on the main thread: `Ok(policy_decision)` (the label to audit; `None` when the
/// `AgentOpsPolicy` flag is off, so O1's audit shape is unchanged) once the request is allowed, or
/// `Err(reason)` once it is denied. A single continuation, rather than separate `proceed`/`fail`
/// closures, because both would otherwise need to independently own the caller's `oneshot::Sender`
/// (mục 3.6 of the plan allows restructuring the closure shape when `'static` makes it awkward).
fn authorize(
    input: AuthorizeInput,
    continue_with: impl FnOnce(
        Result<AuthorizedOutcome, AgentBridgeError>,
        &mut ModelContext<LocalControlBridge>,
    ) + 'static,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    if !FeatureFlag::AgentOpsPolicy.is_enabled() {
        continue_with(
            Ok(AuthorizedOutcome {
                policy_decision: None,
                agent_id: None,
            }),
            ctx,
        );
        return;
    }
    ctx.spawn(
        async move {
            let home = dirs::home_dir();
            let agent_id = home
                .as_deref()
                .and_then(|home| resolve_agent_id(home, input.token_sha256.as_deref()));
            let decision = evaluate_policy(
                home.as_deref(),
                &input.hostname,
                input.subject.as_policy_request(),
                agent_id.is_some(),
            );
            (input, decision, agent_id)
        },
        move |_, (mut input, decision, agent_id), ctx| {
            input.agent.agent_id = agent_id.clone();
            input.target.agent_id = agent_id;
            match decision {
                Decision::Allow => continue_with(
                    Ok(AuthorizedOutcome {
                        policy_decision: Some("allow"),
                        agent_id: input.agent.agent_id,
                    }),
                    ctx,
                ),
                Decision::Ask => ask_authorization(input, continue_with, ctx),
                Decision::Deny(reason) => {
                    deny_authorization(input, "deny", reason, continue_with, ctx)
                }
            }
        },
    );
}

/// Handles `Decision::Ask` (mục 3.6 of the O2 plan): runs the request immediately if the session
/// was trusted ("Trust this session for the rest of the attachment") or its exact command was
/// already allowed for the rest of this session ("Allow this command in this session"), otherwise
/// queues an [`ApprovalRequest`] and waits up to `APPROVAL_TIMEOUT_SECS` for a person to decide on
/// it. `Decision::Deny` is decided by the policy in `authorize` before this function is ever
/// called, so a trusted or allowed session can never bypass a `[deny]` rule.
fn ask_authorization(
    input: AuthorizeInput,
    continue_with: impl FnOnce(
        Result<AuthorizedOutcome, AgentBridgeError>,
        &mut ModelContext<LocalControlBridge>,
    ) + 'static,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    let trusted =
        AgentBridgeModel::handle(ctx).read(ctx, |model, _| model.is_session_trusted(input.session));
    if trusted {
        continue_with(
            Ok(AuthorizedOutcome {
                policy_decision: Some("session_trusted"),
                agent_id: input.agent.agent_id,
            }),
            ctx,
        );
        return;
    }

    if let Some(command) = input.subject.command() {
        let allowed = AgentBridgeModel::handle(ctx).read(ctx, |model, _| {
            model.is_command_allowed_in_session(input.session, command)
        });
        if allowed {
            continue_with(
                Ok(AuthorizedOutcome {
                    policy_decision: Some("session_rule"),
                    agent_id: input.agent.agent_id,
                }),
                ctx,
            );
            return;
        }
    }

    let request = ApprovalRequest {
        request_id: input.target.request_id,
        session: Some(input.session),
        session_label: format!(
            "{}@{}",
            input.target.session.user, input.target.session.host
        ),
        agent: input.agent.clone(),
        subject: input
            .subject
            .clone()
            .into_approval_subject(input.target.cwd.clone()),
        deadline: SystemTime::now() + Duration::from_secs(APPROVAL_TIMEOUT_SECS),
        window_id: input.window_id,
    };
    let request_id = request.request_id;
    let pushed =
        AgentBridgeModel::handle(ctx).update(ctx, |model, ctx| model.push_approval(request, ctx));
    let receiver = match pushed {
        Ok(receiver) => receiver,
        Err(error) => {
            continue_with(Err(error), ctx);
            return;
        }
    };

    ctx.spawn(
        async move {
            // Fail-closed (mục 3.6): if the request cannot be recorded, it must not sit in the
            // queue silently waiting for a decision no one can later account for.
            let audited = ops::audit_approval_requested(
                &input.target,
                input.action,
                input.agent.claimed.as_deref(),
                input.subject.command(),
                input.subject.path(),
            );
            match audited {
                Ok(()) => {
                    let decision = approval::wait_for_decision(
                        receiver,
                        Duration::from_secs(APPROVAL_TIMEOUT_SECS),
                    )
                    .await;
                    (input, Ok(decision))
                }
                Err(error) => (input, Err(error)),
            }
        },
        move |_, (input, outcome), ctx| match outcome {
            Ok(decision) => finish_ask(input, decision, continue_with, ctx),
            Err(error) => {
                AgentBridgeModel::handle(ctx)
                    .update(ctx, |model, ctx| model.remove_approval(request_id, ctx));
                continue_with(Err(error), ctx);
            }
        },
    );
}

/// Turns a decision out of the approval queue into the `authorize` outcome. `Approve`/
/// `AllowInSession` re-check the attachment first — a detach or expiry while the request waited
/// must still stop it from running — and `AllowInSession` also remembers the command for the rest
/// of the session. Every other decision denies the request, each with its own audited reason
/// (mục 3.3/3.10 of the plan).
fn finish_ask(
    input: AuthorizeInput,
    decision: ApprovalDecision,
    continue_with: impl FnOnce(
        Result<AuthorizedOutcome, AgentBridgeError>,
        &mut ModelContext<LocalControlBridge>,
    ) + 'static,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    match decision {
        ApprovalDecision::Approve | ApprovalDecision::AllowInSession => {
            let recheck = AgentBridgeModel::handle(ctx).update(ctx, |model, _| {
                model.check(
                    input.session,
                    Access::Full,
                    &input.target.session.user,
                    &input.target.session.host,
                )
            });
            if let Err(error) = recheck {
                continue_with(Err(error), ctx);
                return;
            }
            if decision == ApprovalDecision::AllowInSession
                && let Some(command) = input.subject.command().map(str::to_owned)
            {
                AgentBridgeModel::handle(ctx).update(ctx, |model, _| {
                    model.allow_command_in_session(input.session, &command)
                });
            }
            let label = if decision == ApprovalDecision::AllowInSession {
                "approved_in_session"
            } else {
                "approved"
            };
            continue_with(
                Ok(AuthorizedOutcome {
                    policy_decision: Some(label),
                    agent_id: input.agent.agent_id,
                }),
                ctx,
            );
        }
        ApprovalDecision::Deny => deny_authorization(
            input,
            "ask_denied",
            "the user denied it.".to_owned(),
            continue_with,
            ctx,
        ),
        ApprovalDecision::TimedOut => {
            let request_id = input.target.request_id;
            AgentBridgeModel::handle(ctx)
                .update(ctx, |model, ctx| model.remove_approval(request_id, ctx));
            deny_authorization(
                input,
                "ask_timeout",
                "no one approved it within 5 minutes.".to_owned(),
                continue_with,
                ctx,
            );
        }
        ApprovalDecision::Revoked => deny_authorization(
            input,
            "revoked",
            "the user revoked agent access while it was waiting.".to_owned(),
            continue_with,
            ctx,
        ),
    }
}

fn deny_authorization(
    input: AuthorizeInput,
    policy_decision: &'static str,
    reason: String,
    continue_with: impl FnOnce(
        Result<AuthorizedOutcome, AgentBridgeError>,
        &mut ModelContext<LocalControlBridge>,
    ),
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    ops::audit_policy_denied(
        &input.target,
        input.action,
        input.agent.claimed.as_deref(),
        input.subject.command(),
        input.subject.path(),
        policy_decision,
        &reason,
    );
    continue_with(Err(AgentBridgeError::PolicyDenied(reason)), ctx);
}

/// Loads `~/.warp/agent-ops/policy.toml` and evaluates `request` against it. A policy that cannot
/// be loaded, or a home directory that cannot be found, denies every write (fail-closed, mục 2.3
/// of the plan).
fn evaluate_policy(
    home: Option<&Path>,
    hostname: &str,
    request: PolicyRequest<'_>,
    paired: bool,
) -> Decision {
    let Some(home) = home else {
        return Decision::Deny("the user's home directory could not be found.".to_owned());
    };
    match policy::load(home) {
        Ok(policy) => policy.evaluate(hostname, request, paired),
        Err(error) => Decision::Deny(format!(
            "the policy file ~/.warp/agent-ops/policy.toml is invalid ({error}). Ask the user to \
             fix it."
        )),
    }
}

/// Resolves `token_sha256` (the hash of a request's `agent_token`, if it had one) against
/// `~/.warp/agent-ops/agents.toml`, giving the paired agent's id (mục 3.11 of the plan). `None` for
/// an untokened or unpaired client — that is not an error, just an unverified caller.
fn resolve_agent_id(home: &Path, token_sha256: Option<&str>) -> Option<String> {
    let token_sha256 = token_sha256?;
    let agents = pairing::load(home);
    pairing::find(&agents, token_sha256).map(|agent| agent.id.clone())
}

pub(super) fn ensure_enabled(kind: ActionKind) -> Result<(), ControlError> {
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
                terminal_view: terminal.clone(),
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
