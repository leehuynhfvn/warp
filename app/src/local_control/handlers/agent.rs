//! `agent.*` actions: manage paired agent clients (mục 3.11 of the O2 agent-ops policy plan).

use std::time::{Duration, SystemTime};

use ::local_control::protocol::{
    APPROVAL_TIMEOUT_SECS, AgentPairParams, AgentPairResult, AgentPairStatus, RequestEnvelope,
};
use ::local_control::{ActionKind, ControlError, ErrorCode};
use futures::channel::oneshot;
use warpui::{ModelContext, SingletonEntity as _};

use super::remote::{self, RemoteReceiver};
use crate::agent_bridge::approval::{
    AgentLabel, ApprovalDecision, ApprovalRequest, ApprovalSubject,
};
use crate::agent_bridge::error::AgentBridgeError;
use crate::agent_bridge::model::AgentBridgeModel;
use crate::agent_bridge::ops::validate_agent;
use crate::agent_bridge::{approval, pairing};
use crate::local_control::LocalControlBridge;
use crate::local_control::resolver::target_window_id_for_target;

/// Pairs the calling agent client with Warp, asking the user to approve it the first time. An
/// already-paired token answers immediately with `already_paired`; a never-seen one queues an
/// [`ApprovalRequest`] and waits up to `APPROVAL_TIMEOUT_SECS`, the same as any other write the
/// agent-ops policy asks about — except pairing is not tied to a session (mục 3.11 of the plan).
pub(crate) fn pair(
    request: &RequestEnvelope,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<RemoteReceiver, ControlError> {
    let kind = ActionKind::AgentPair;
    remote::ensure_enabled(kind)?;
    let params = request.action.params_as::<AgentPairParams>()?;
    validate_agent(Some(&params.name)).map_err(ControlError::from)?;
    let Some(token) = &request.agent_token else {
        return Err(ControlError::new(
            ErrorCode::InvalidParams,
            "agent.pair requires an agent_token in the request envelope",
        ));
    };
    let token_sha256 = pairing::hash(token.secret());
    let Some(home) = dirs::home_dir() else {
        return Err(ControlError::new(
            ErrorCode::RemoteOperationFailed,
            "the user's home directory could not be found",
        ));
    };

    if let Some(agent) = pairing::find(&pairing::load(&home), &token_sha256) {
        return respond_immediately(AgentPairResult {
            agent_id: agent.id.clone(),
            status: AgentPairStatus::AlreadyPaired,
        });
    }

    let busy = AgentBridgeModel::handle(ctx).read(ctx, |model, _| model.has_pending_pairing());
    if busy {
        return Err(ControlError::new(
            ErrorCode::SessionBusy,
            "another agent is waiting to be paired",
        ));
    }

    let window_id = target_window_id_for_target(ctx, &request.target, kind)?;
    let approval_request = ApprovalRequest {
        request_id: request.request_id,
        session: None,
        session_label: String::new(),
        agent: AgentLabel {
            claimed: Some(params.name.clone()),
            agent_id: None,
        },
        subject: ApprovalSubject::Pairing {
            name: params.name.clone(),
        },
        deadline: SystemTime::now() + Duration::from_secs(APPROVAL_TIMEOUT_SECS),
        window_id,
    };
    let receiver = AgentBridgeModel::handle(ctx)
        .update(ctx, |model, ctx| model.push_approval(approval_request, ctx))
        .map_err(ControlError::from)?;

    let (sender, result_receiver) = oneshot::channel();
    let name = params.name;
    let request_id = request.request_id;
    ctx.spawn(
        async move {
            let decision =
                approval::wait_for_decision(receiver, Duration::from_secs(APPROVAL_TIMEOUT_SECS))
                    .await;
            let outcome = match decision {
                ApprovalDecision::Approve | ApprovalDecision::AllowInSession => {
                    pairing::add(&home, &name, token_sha256)
                        .map(|agent| AgentPairResult {
                            agent_id: agent.id,
                            status: AgentPairStatus::Paired,
                        })
                        .map_err(|err| AgentBridgeError::Io(format!("could not pair it: {err}")))
                }
                ApprovalDecision::Deny | ApprovalDecision::TimedOut | ApprovalDecision::Revoked => {
                    Err(AgentBridgeError::PolicyDenied(
                        "the user did not pair this agent".to_owned(),
                    ))
                }
            };
            (decision, outcome)
        },
        move |_, (decision, outcome), ctx| {
            // `decide`/`revoke_*` already removed the entry for every other decision; only a
            // timeout leaves it in the queue for the completion side to clean up (mirrors
            // `remote::finish_ask`'s `TimedOut` arm).
            if decision == ApprovalDecision::TimedOut {
                AgentBridgeModel::handle(ctx)
                    .update(ctx, |model, ctx| model.remove_approval(request_id, ctx));
            }
            let value = outcome.and_then(to_json);
            if sender.send(value.map_err(ControlError::from)).is_err() {
                log::debug!("A local-control client stopped waiting for an agent.pair result");
            }
        },
    );
    Ok(result_receiver)
}

fn respond_immediately(result: AgentPairResult) -> Result<RemoteReceiver, ControlError> {
    let (sender, receiver) = oneshot::channel();
    if sender
        .send(to_json(result).map_err(ControlError::from))
        .is_err()
    {
        log::debug!("A local-control client stopped waiting for an agent.pair result");
    }
    Ok(receiver)
}

fn to_json(result: AgentPairResult) -> Result<serde_json::Value, AgentBridgeError> {
    serde_json::to_value(result)
        .map_err(|err| AgentBridgeError::Io(format!("could not encode the result: {err}")))
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
