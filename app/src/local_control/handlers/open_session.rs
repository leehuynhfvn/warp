//! `remote.session.open` and `remote.session.close`: an agent that has been paired with Warp opens
//! an SSH session to a server of the user's directory in a new tab and gets it attached, and closes
//! the sessions it opened itself.
//!
//! Nothing is opened unless the agent-ops policy allows it or the user approves it, and every
//! command the agent runs in the session afterwards still goes through the policy that decides
//! writes (`remote::authorize`): opening a session hands the agent a door, not a licence.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use ::local_control::protocol::{
    APPROVAL_TIMEOUT_SECS, MAX_OPEN_PURPOSE_BYTES, OPEN_WAIT_DEFAULT_SECS, OPEN_WAIT_MAX_SECS,
    RemoteAccess, RemoteOpenElevation, RemoteOpenStatus, RemoteSessionOpenParams,
    RemoteSessionOpenResult, RequestEnvelope, TargetSelector,
};
use ::local_control::{ActionKind, ControlError, ErrorCode};
use futures::channel::oneshot;
use futures::future::Either;
use instant::Instant;
use serde_json::Value;
use settings::Setting as _;
use uuid::Uuid;
use warp_util::path::ShellFamily;
use warpui::r#async::Timer;
use warpui::{ModelContext, SingletonEntity, ViewHandle, WindowId};

use super::hosts::{RESOLVE_BUDGET, resolve_connection};
use super::metadata::session_entries;
use super::remote::{RemoteReceiver, ensure_enabled, resolve_agent_id};
use crate::agent_bridge::OPEN_PENDING_TTL;
use crate::agent_bridge::approval::{
    self, AgentLabel, ApprovalDecision, ApprovalRequest, ApprovalSubject,
};
use crate::agent_bridge::attachments::Access;
use crate::agent_bridge::audit::audit_dir;
use crate::agent_bridge::error::AgentBridgeError;
use crate::agent_bridge::model::{AgentBridgeModel, OpenReady};
use crate::agent_bridge::open_audit::{OpenRequest as AuditRequest, SessionRequestAudit};
use crate::agent_bridge::opened::{ElevationPlan, OpenState, Opened, SUDO_COMMAND, elevation_plan};
use crate::agent_bridge::ops::validate_agent;
use crate::agent_bridge::policy::{self, Decision, OpenLimits, OpenRequest as PolicyOpenRequest};
use crate::features::FeatureFlag;
use crate::host_directory::{
    Host, HostDirectoryModel, RootLogin, SshResolver, SystemSsh, validate_alias,
};
use crate::local_control::LocalControlBridge;
use crate::local_control::resolver::{target_window_id_for_target, workspace_for_window};
use crate::terminal::warpify::settings::WarpifySettings;
use crate::workspace::Workspace;

pub(super) type Sender = oneshot::Sender<Result<Value, ControlError>>;

const NOT_PAIRED: &str = "opening a session needs an agent that is paired with Warp. Ask the \
                          user to approve pairing, or run this agent's MCP server without \
                          --no-pair.";

/// A request that passed validation, with the agent's words for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidOpen {
    alias: String,
    access: Access,
    purpose: String,
    root: bool,
    wait: Duration,
    agent: Option<String>,
}

/// Everything about one `remote.session.open` that its steps hand on to each other.
struct OpenFlow {
    request_id: Uuid,
    valid: ValidOpen,
    window_id: WindowId,
    workspace: ViewHandle<Workspace>,
    host: Host,
    plan: ElevationPlan,
    sender: Sender,
}

/// What the background step learned before the main thread decides.
struct Staged {
    agent_id: Option<String>,
    decision: Decision,
    limits: OpenLimits,
    connection: Option<String>,
}

pub(crate) fn open(
    request: &RequestEnvelope,
    token_sha256: Option<String>,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<RemoteReceiver, ControlError> {
    let kind = ActionKind::RemoteSessionOpen;
    ensure_open_enabled(kind)?;
    let params = request.action.params_as::<RemoteSessionOpenParams>()?;
    let valid = validate_open(params)?;
    let host = find_host(&valid.alias, ctx)?;
    ensure_warpify_will_take_it(&valid.alias, ctx)?;
    let sudo_is_warpifiable = WarpifySettings::as_ref(ctx)
        .is_compatible_subshell_command(SUDO_COMMAND, ShellFamily::Posix);
    let plan = elevation_plan(valid.root, host.root_login, sudo_is_warpifiable);
    let window_id = target_window_id_for_target(ctx, &request.target, kind)?;
    let workspace = workspace_for_window(window_id, kind, ctx)?;

    let (sender, receiver) = oneshot::channel();
    let flow = OpenFlow {
        request_id: request.request_id,
        valid,
        window_id,
        workspace,
        host,
        plan,
        sender,
    };
    let stage_host = flow.host.clone();
    let stage_request = staging_request(&flow.valid, &flow.host, &flow.plan);
    ctx.spawn(
        async move {
            stage(
                dirs::home_dir().as_deref(),
                &SystemSsh,
                &stage_host,
                &stage_request,
                token_sha256.as_deref(),
            )
        },
        move |_, staged, ctx| decide(flow, staged, ctx),
    );
    Ok(receiver)
}

pub(super) fn ensure_open_enabled(kind: ActionKind) -> Result<(), ControlError> {
    ensure_enabled(kind)?;
    let missing = [
        (FeatureFlag::AgentOpsPolicy, "the agent-ops policy"),
        (FeatureFlag::AgentOpsHosts, "the server directory"),
        (FeatureFlag::AgentOpsOpenSession, "agents opening sessions"),
    ]
    .into_iter()
    .find(|(flag, _)| !flag.is_enabled());
    match missing {
        Some((_, name)) => Err(ControlError::new(
            ErrorCode::UnsupportedAction,
            format!(
                "{} requires {name}, which is not enabled in this build",
                kind.as_str()
            ),
        )),
        None => Ok(()),
    }
}

/// Checks the request's own fields; nothing outside it is looked at.
fn validate_open(params: RemoteSessionOpenParams) -> Result<ValidOpen, ControlError> {
    let invalid = |message: String| ControlError::new(ErrorCode::InvalidParams, message);
    validate_alias(&params.host)
        .map_err(|reason| invalid(format!("host \"{}\" {reason}", params.host)))?;
    validate_agent(params.agent.as_deref()).map_err(ControlError::from)?;
    let purpose = params.purpose.trim();
    if purpose.is_empty() {
        return Err(invalid(
            "purpose is empty; say why the session is needed".to_owned(),
        ));
    }
    if purpose.len() > MAX_OPEN_PURPOSE_BYTES {
        return Err(invalid(format!(
            "purpose is longer than {MAX_OPEN_PURPOSE_BYTES} bytes"
        )));
    }
    if purpose.chars().any(char::is_control) {
        return Err(invalid(
            "purpose may not contain control characters or line breaks".to_owned(),
        ));
    }
    let wait_secs = params.wait_secs.unwrap_or(OPEN_WAIT_DEFAULT_SECS);
    if !(1..=OPEN_WAIT_MAX_SECS).contains(&wait_secs) {
        return Err(invalid(format!(
            "wait_secs must be between 1 and {OPEN_WAIT_MAX_SECS}"
        )));
    }
    Ok(ValidOpen {
        alias: params.host,
        access: match params.access {
            RemoteAccess::ReadOnly => Access::ReadOnly,
            RemoteAccess::Full => Access::Full,
        },
        purpose: purpose.to_owned(),
        root: params.root,
        wait: Duration::from_secs(wait_secs.into()),
        agent: params.agent,
    })
}

/// The server the agent named; only one the user's directory holds, and still has in their SSH
/// configuration, can be opened.
fn find_host(
    alias: &str,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<Host, ControlError> {
    let host = HostDirectoryModel::as_ref(ctx)
        .hosts()
        .iter()
        .find(|host| host.alias == alias)
        .cloned();
    match host {
        None => Err(ControlError::new(
            ErrorCode::InvalidParams,
            format!("there is no server \"{alias}\" in the user's server list; see list_hosts"),
        )),
        Some(host) if host.missing => Err(ControlError::new(
            ErrorCode::InvalidParams,
            format!("\"{alias}\" is no longer in the user's SSH configuration"),
        )),
        Some(host) => Ok(host),
    }
}

/// Waiting for a session that Warp will not Warpify only wastes the agent's time, so the settings
/// that stop it are checked first. The user can still change them, in which case the request
/// simply is not answered `ready`.
fn ensure_warpify_will_take_it(
    alias: &str,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<(), ControlError> {
    let settings = WarpifySettings::as_ref(ctx);
    let reason = if !*settings.enable_ssh_warpification.value() {
        Some("SSH Warpify is turned off in Settings > Warpify".to_owned())
    } else if settings.is_ssh_host_denylisted(alias) {
        Some(format!(
            "{alias} is in the list of hosts Warpify skips (Settings > Warpify)"
        ))
    } else {
        None
    };
    match reason {
        Some(reason) => Err(ControlError::new(
            ErrorCode::RemoteOperationFailed,
            format!("The session would not be Warpified, so it could not be attached: {reason}."),
        )),
        None => Ok(()),
    }
}

struct StagingRequest {
    alias: String,
    tags: Vec<String>,
    access: Access,
    root: bool,
}

fn staging_request(valid: &ValidOpen, host: &Host, plan: &ElevationPlan) -> StagingRequest {
    // A session that is root from the first byte counts as root even when the agent did not ask.
    let ends_as_root = host.root_login == RootLogin::Root || *plan == ElevationPlan::Run;
    StagingRequest {
        alias: valid.alias.clone(),
        tags: host.tags.clone(),
        access: valid.access,
        root: ends_as_root,
    }
}

/// The part that reads files and runs `ssh -G`, so it happens off the main thread.
fn stage(
    home: Option<&Path>,
    resolver: &dyn SshResolver,
    host: &Host,
    request: &StagingRequest,
    token_sha256: Option<&str>,
) -> Staged {
    let agent_id = home.and_then(|home| resolve_agent_id(home, token_sha256));
    if agent_id.is_none() {
        // Nothing else is worth reading, and `ssh -G` is not run for an agent that is not paired.
        return Staged {
            agent_id: None,
            decision: Decision::Deny(NOT_PAIRED.to_owned()),
            limits: OpenLimits::default(),
            connection: None,
        };
    }
    let (decision, limits) = evaluate_open_policy(
        home,
        PolicyOpenRequest {
            alias: &request.alias,
            tags: &request.tags,
            access: request.access,
            root: request.root,
        },
    );
    let connection = (decision == Decision::Ask)
        .then(|| resolve_connection(host, resolver, Instant::now() + RESOLVE_BUDGET))
        .flatten();
    Staged {
        agent_id,
        decision,
        limits,
        connection,
    }
}

/// Loads `~/.warp/agent-ops/policy.toml` and decides about opening. A policy that cannot be
/// loaded refuses everything, like it does for writes.
fn evaluate_open_policy(
    home: Option<&Path>,
    request: PolicyOpenRequest<'_>,
) -> (Decision, OpenLimits) {
    let Some(home) = home else {
        return (
            Decision::Deny("the user's home directory could not be found.".to_owned()),
            OpenLimits::default(),
        );
    };
    match policy::load(home) {
        Ok(policy) => (policy.evaluate_open(request), policy.open_limits()),
        Err(error) => (
            Decision::Deny(policy::invalid_policy_reason(&error)),
            OpenLimits::default(),
        ),
    }
}

fn audit_for(flow: &OpenFlow, agent_id: Option<&str>) -> SessionRequestAudit {
    SessionRequestAudit::begin(
        audit_dir(),
        AuditRequest {
            action: ActionKind::RemoteSessionOpen,
            request_id: flow.request_id,
            agent: flow.valid.agent.as_deref(),
            agent_id,
            alias: &flow.valid.alias,
            purpose: Some(&flow.valid.purpose),
            access: Some(flow.valid.access),
        },
    )
}

pub(super) fn answer(sender: Sender, result: Result<Value, ControlError>) {
    if sender.send(result).is_err() {
        log::debug!("A local-control client stopped waiting for a session to open");
    }
}

/// Ends the request with `error`, after writing it to the audit log with what the policy decided.
fn refuse(
    flow: OpenFlow,
    audit: SessionRequestAudit,
    policy_decision: &'static str,
    error: AgentBridgeError,
) {
    audit.with_policy(policy_decision, None).finish(Err(&error));
    answer(flow.sender, Err(error.into()));
}

/// Main thread, after the background step: the pairing, the policy and the limits decide whether
/// to open, ask, or refuse.
fn decide(flow: OpenFlow, staged: Staged, ctx: &mut ModelContext<LocalControlBridge>) {
    let audit = audit_for(&flow, staged.agent_id.as_deref());
    let Some(agent_id) = staged.agent_id else {
        let reason = NOT_PAIRED.to_owned();
        refuse(flow, audit, "deny", AgentBridgeError::PolicyDenied(reason));
        return;
    };
    if let Decision::Deny(reason) = staged.decision {
        refuse(flow, audit, "deny", AgentBridgeError::PolicyDenied(reason));
        return;
    }
    if let Err(error) = check_limits(&flow, &agent_id, staged.limits, ctx) {
        refuse(flow, audit, "deny", error);
        return;
    }
    match staged.decision {
        Decision::Deny(_) => {}
        Decision::Allow => start(flow, audit, agent_id, "allow", ctx),
        Decision::Ask => ask(flow, audit, agent_id, staged.connection, staged.limits, ctx),
    }
}

/// The panes that hold a terminal session right now, so limits count only sessions still open.
fn live_panes(ctx: &mut ModelContext<LocalControlBridge>) -> HashSet<String> {
    session_entries(
        &TargetSelector::default(),
        ActionKind::RemoteSessionList,
        ctx,
    )
    .map(|entries| {
        entries
            .iter()
            .map(|entry| entry.pane_id.to_string())
            .collect()
    })
    .unwrap_or_default()
}

fn check_limits(
    flow: &OpenFlow,
    agent_id: &str,
    limits: OpenLimits,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<(), AgentBridgeError> {
    let live = live_panes(ctx);
    AgentBridgeModel::handle(ctx)
        .update(ctx, |model, _| {
            model.check_open_limits(agent_id, &flow.valid.alias, limits, &live)
        })
        .map_err(|error| AgentBridgeError::PolicyDenied(error.to_string()))
}

/// Queues the request for the user to decide on, then waits for the decision (mục 3.6 of the O2
/// plan, for a request that has no session yet).
fn ask(
    flow: OpenFlow,
    audit: SessionRequestAudit,
    agent_id: String,
    connection: Option<String>,
    limits: OpenLimits,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    let request = ApprovalRequest {
        request_id: flow.request_id,
        session: None,
        session_label: String::new(),
        agent: AgentLabel {
            claimed: flow.valid.agent.clone(),
            agent_id: Some(agent_id.clone()),
        },
        subject: ApprovalSubject::OpenSession {
            alias: flow.valid.alias.clone(),
            access: flow.valid.access,
            purpose: flow.valid.purpose.clone(),
            tags: flow.host.tags.clone(),
            connection,
            elevation: flow.plan.clone(),
        },
        deadline: SystemTime::now() + Duration::from_secs(APPROVAL_TIMEOUT_SECS),
        window_id: flow.window_id,
    };
    let request_id = flow.request_id;
    let pushed =
        AgentBridgeModel::handle(ctx).update(ctx, |model, ctx| model.push_approval(request, ctx));
    let receiver = match pushed {
        Ok(receiver) => receiver,
        Err(error) => {
            refuse(flow, audit, "deny", error);
            return;
        }
    };
    ctx.spawn(
        async move {
            // Fail-closed: a request nobody could later account for does not wait for a decision.
            match audit.record_approval_requested() {
                Ok(()) => {
                    let decision = approval::wait_for_decision(
                        receiver,
                        Duration::from_secs(APPROVAL_TIMEOUT_SECS),
                    )
                    .await;
                    (audit, Ok(decision))
                }
                Err(error) => (audit, Err(error)),
            }
        },
        move |_, (audit, outcome), ctx| match outcome {
            Ok(decision) => finish_ask(flow, audit, agent_id, decision, limits, ctx),
            Err(error) => {
                AgentBridgeModel::handle(ctx)
                    .update(ctx, |model, ctx| model.remove_approval(request_id, ctx));
                refuse(flow, audit, "deny", error);
            }
        },
    );
}

fn finish_ask(
    flow: OpenFlow,
    audit: SessionRequestAudit,
    agent_id: String,
    decision: ApprovalDecision,
    limits: OpenLimits,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    let (label, reason) = match decision {
        ApprovalDecision::Approve | ApprovalDecision::AllowInSession => {
            // Another session may have been opened, or the limits lowered, while the user decided.
            if let Err(error) = check_limits(&flow, &agent_id, limits, ctx) {
                refuse(flow, audit, "deny", error);
                return;
            }
            start(flow, audit, agent_id, "approved", ctx);
            return;
        }
        ApprovalDecision::Deny => ("ask_denied", "the user denied it."),
        ApprovalDecision::TimedOut => {
            let request_id = flow.request_id;
            AgentBridgeModel::handle(ctx)
                .update(ctx, |model, ctx| model.remove_approval(request_id, ctx));
            ("ask_timeout", "no one approved it within 5 minutes.")
        }
        ApprovalDecision::Revoked => (
            "revoked",
            "the user revoked agent access while it was waiting.",
        ),
    };
    refuse(
        flow,
        audit,
        label,
        AgentBridgeError::PolicyDenied(reason.to_owned()),
    );
}

/// Writes the "started" line, then opens the tab. Nothing is opened for a request whose audit
/// line could not be written.
fn start(
    flow: OpenFlow,
    audit: SessionRequestAudit,
    agent_id: String,
    policy_decision: &'static str,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    let audit = audit.with_policy(policy_decision, None);
    ctx.spawn(
        async move {
            let started = audit.record_start();
            (audit, started)
        },
        move |_, (audit, started), ctx| match started {
            Ok(()) => open_tab(flow, audit, agent_id, ctx),
            Err(error) => refuse(flow, audit, policy_decision, error),
        },
    );
}

fn open_tab(
    flow: OpenFlow,
    audit: SessionRequestAudit,
    agent_id: String,
    ctx: &mut ModelContext<LocalControlBridge>,
) {
    let alias = flow.valid.alias.clone();
    let opened_tab = flow.workspace.update(ctx, |workspace, ctx| {
        workspace.open_agent_session_tab(&alias, ctx)
    });
    let tab = match opened_tab {
        Ok(tab) => tab,
        Err(message) => {
            let error =
                AgentBridgeError::Executor(format!("the tab could not be opened: {message}"));
            refuse(flow, audit, "error", error);
            return;
        }
    };
    let pane = tab.pane_id.to_string();
    let opened = Opened {
        agent_id,
        alias: flow.valid.alias.clone(),
        access: flow.valid.access,
        elevate: flow.plan == ElevationPlan::Run,
        opened_at: Instant::now(),
        state: OpenState::Connecting,
    };
    let sessions = tab
        .terminal_view
        .read(ctx, |view, _| view.sessions_model().clone());
    let ready = AgentBridgeModel::handle(ctx).update(ctx, |model, ctx| {
        model.track_open(
            pane.clone(),
            opened,
            &sessions,
            Some(tab.terminal_view.downgrade()),
            ctx,
        )
    });
    let wait = flow.valid.wait;
    ctx.spawn(
        async move { wait_until_ready(ready, wait).await },
        move |_, ready, _| {
            let result = open_result(&flow.valid, &flow.plan, &pane, ready);
            audit
                .with_session(&pane, result.user.as_deref().unwrap_or_default())
                .finish(Ok(()));
            answer(
                flow.sender,
                serde_json::to_value(result).map_err(|err| {
                    ControlError::with_details(
                        ErrorCode::Internal,
                        "failed to serialize the opened session",
                        err.to_string(),
                    )
                }),
            );
        },
    );
}

async fn wait_until_ready(
    ready: oneshot::Receiver<OpenReady>,
    wait: Duration,
) -> Option<OpenReady> {
    let timer = Timer::after(wait);
    futures::pin_mut!(ready);
    futures::pin_mut!(timer);
    match futures::future::select(ready, timer).await {
        Either::Left((Ok(ready), _)) => Some(ready),
        Either::Left((Err(_), _)) | Either::Right(_) => None,
    }
}

/// What the agent is told: `ready` with who it is signed in as, or `connecting` when the session
/// did not finish in time and may still attach by itself (`OPEN_PENDING_TTL`).
fn open_result(
    valid: &ValidOpen,
    plan: &ElevationPlan,
    pane: &str,
    ready: Option<OpenReady>,
) -> RemoteSessionOpenResult {
    let (elevation, elevation_note) = describe_elevation(plan, ready.as_ref());
    let access = match valid.access {
        Access::ReadOnly => RemoteAccess::ReadOnly,
        Access::Full => RemoteAccess::Full,
    };
    let (status, host, user, note) = match ready {
        Some(ready) => (
            RemoteOpenStatus::Ready,
            Some(ready.host),
            Some(ready.user),
            elevation_note,
        ),
        None => (
            RemoteOpenStatus::Connecting,
            None,
            None,
            Some(connecting_note(elevation_note)),
        ),
    };
    RemoteSessionOpenResult {
        status,
        session_id: pane.to_owned(),
        host_alias: valid.alias.clone(),
        host,
        user,
        access,
        elevation,
        note,
    }
}

fn describe_elevation(
    plan: &ElevationPlan,
    ready: Option<&OpenReady>,
) -> (RemoteOpenElevation, Option<String>) {
    match plan {
        ElevationPlan::NotRequested => (RemoteOpenElevation::NotRequested, None),
        ElevationPlan::AlreadyRoot => (RemoteOpenElevation::AlreadyRoot, None),
        ElevationPlan::Skipped(reason) => (
            RemoteOpenElevation::Skipped,
            Some(format!("Not root: {reason}")),
        ),
        ElevationPlan::Run => match ready {
            None => (RemoteOpenElevation::Pending, None),
            Some(ready) if ready.elevated => (RemoteOpenElevation::Elevated, None),
            Some(ready) => (RemoteOpenElevation::Skipped, ready.root_note.clone()),
        },
    }
}

fn connecting_note(elevation_note: Option<String>) -> String {
    let minutes = OPEN_PENDING_TTL.as_secs() / 60;
    let mut note = format!(
        "The tab is open but the session is not ready yet. The user may have to answer something \
         in it (a password, a passphrase, a host key). It is attached by itself if it becomes \
         ready within {minutes} minutes; check with list_sessions. Do not open another one \
         meanwhile."
    );
    if let Some(elevation_note) = elevation_note {
        note.push(' ');
        note.push_str(&elevation_note);
    }
    note
}

#[cfg(test)]
#[path = "open_session_tests.rs"]
mod tests;
