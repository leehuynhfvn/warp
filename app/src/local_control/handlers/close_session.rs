//! `remote.session.close`: an agent closes a session that it opened itself.

use ::local_control::protocol::{
    RemoteSessionCloseParams, RemoteSessionCloseResult, RequestEnvelope, SessionTarget,
};
use ::local_control::{ActionKind, ControlError, ErrorCode};
use futures::channel::oneshot;
use serde_json::Value;
use uuid::Uuid;
use warpui::{ModelContext, SingletonEntity};

use super::metadata::session_entries;
use super::open_session::{answer, ensure_open_enabled};
use super::remote::{RemoteReceiver, resolve_agent_id};
use crate::agent_bridge::audit::audit_dir;
use crate::agent_bridge::error::AgentBridgeError;
use crate::agent_bridge::model::AgentBridgeModel;
use crate::agent_bridge::open_audit::{OpenRequest as AuditRequest, SessionRequestAudit};
use crate::agent_bridge::ops::validate_agent;
use crate::local_control::LocalControlBridge;

/// Closes a session that the calling agent opened. A session the user opened by hand, or another
/// agent opened, is not the caller's to close.
pub(crate) fn close(
    request: &RequestEnvelope,
    token_sha256: Option<String>,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<RemoteReceiver, ControlError> {
    let kind = ActionKind::RemoteSessionClose;
    ensure_open_enabled(kind)?;
    let params = request.action.params_as::<RemoteSessionCloseParams>()?;
    validate_agent(params.agent.as_deref()).map_err(ControlError::from)?;
    if !matches!(request.target.session, Some(SessionTarget::Id { .. })) {
        return Err(ControlError::new(
            ErrorCode::MissingTarget,
            "remote.session.close needs the id of a session; list them with remote.session.list",
        ));
    }
    let entry = session_entries(&request.target, kind, ctx)?
        .into_iter()
        .next()
        .ok_or_else(|| {
            ControlError::new(
                ErrorCode::StaleTarget,
                "remote.session.close cannot find that session any more",
            )
        })?;
    let pane = entry.pane_id.to_string();
    let (sender, receiver) = oneshot::channel();
    let request_id = request.request_id;
    let agent = params.agent;
    ctx.spawn(
        async move {
            let home = dirs::home_dir();
            home.as_deref()
                .and_then(|home| resolve_agent_id(home, token_sha256.as_deref()))
        },
        move |_, agent_id, ctx| {
            let result = close_owned(
                &pane,
                agent_id.as_deref(),
                agent.as_deref(),
                request_id,
                |ctx| {
                    entry.pane_group.update(ctx, |pane_group, ctx| {
                        pane_group.close_pane(entry.pane_id, ctx)
                    });
                },
                ctx,
            );
            answer(sender, result);
        },
    );
    Ok(receiver)
}

/// Checks that `agent_id` opened `pane`, writes the audit lines, and only then closes it.
fn close_owned(
    pane: &str,
    agent_id: Option<&str>,
    agent: Option<&str>,
    request_id: Uuid,
    close_pane: impl FnOnce(&mut ModelContext<LocalControlBridge>),
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<Value, ControlError> {
    let refused =
        |reason: &str| ControlError::from(AgentBridgeError::PolicyDenied(reason.to_owned()));
    let Some(agent_id) = agent_id else {
        return Err(refused(
            "closing a session needs an agent that is paired with Warp.",
        ));
    };
    let opened = AgentBridgeModel::handle(ctx)
        .read(ctx, |model, _| model.opened_by(pane, agent_id).cloned());
    let Some(opened) = opened else {
        return Err(refused(
            "this session was not opened by you, so it is not yours to close. Sessions the user \
             opened stay open until the user closes them.",
        ));
    };
    let audit = SessionRequestAudit::begin(
        audit_dir(),
        AuditRequest {
            action: ActionKind::RemoteSessionClose,
            request_id,
            agent,
            agent_id: Some(agent_id),
            alias: &opened.alias,
            purpose: None,
            access: None,
        },
    )
    .with_session(pane, "");
    if let Err(error) = audit.record_start() {
        return Err(error.into());
    }
    close_pane(ctx);
    AgentBridgeModel::handle(ctx).update(ctx, |model, ctx| model.forget_opened(pane, ctx));
    audit.finish(Ok(()));
    serde_json::to_value(RemoteSessionCloseResult {
        session_id: pane.to_owned(),
        closed: true,
    })
    .map_err(|err| {
        ControlError::with_details(
            ErrorCode::Internal,
            "failed to serialize the closed session",
            err.to_string(),
        )
    })
}

#[cfg(test)]
#[path = "close_session_tests.rs"]
mod tests;
