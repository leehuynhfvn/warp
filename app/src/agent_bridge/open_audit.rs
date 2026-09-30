//! Audit lines for the requests that open and close a session. They follow the same log and the
//! same rule as every other request (`super::audit`): nothing happens unless its first line could
//! be written. There is no session yet when one is asked for, so `host` is the alias the agent
//! named and `session_id` stays empty until the session has a pane.

use std::path::PathBuf;

use ::local_control::ActionKind;
use chrono::Utc;
use instant::Instant;
use uuid::Uuid;

use super::attachments::Access;
use super::audit::{self, AuditOutcome, AuditRecord};
use super::error::AgentBridgeError;

/// What is known about a session request when it is first written down.
pub(crate) struct OpenRequest<'a> {
    pub(crate) action: ActionKind,
    pub(crate) request_id: Uuid,
    /// The name the client gave itself; never grants anything.
    pub(crate) agent: Option<&'a str>,
    /// The paired identity the request's token resolved to.
    pub(crate) agent_id: Option<&'a str>,
    pub(crate) alias: &'a str,
    pub(crate) purpose: Option<&'a str>,
    pub(crate) access: Option<Access>,
}

pub(crate) struct SessionRequestAudit {
    dir: Option<PathBuf>,
    record: AuditRecord,
    started: Instant,
}

impl SessionRequestAudit {
    pub(crate) fn begin(dir: Option<PathBuf>, request: OpenRequest<'_>) -> Self {
        let record = AuditRecord {
            ts_unix: u64::try_from(Utc::now().timestamp()).unwrap_or_default(),
            request_id: request.request_id,
            agent: request
                .agent
                .filter(|agent| super::ops::validate_agent(Some(agent)).is_ok())
                .map(str::to_owned),
            action: request.action.as_str(),
            session_id: String::new(),
            host: request.alias.to_owned(),
            user: String::new(),
            cwd: None,
            command: None,
            path: None,
            exit_code: None,
            result: AuditOutcome::Ok,
            error_code: None,
            policy_decision: None,
            policy_reason: None,
            agent_id: request.agent_id.map(str::to_owned),
            purpose: request.purpose.map(str::to_owned),
            access: request.access.map(access_label),
            duration_ms: 0,
            bytes: None,
            still_running: false,
        };
        Self {
            dir,
            record,
            started: Instant::now(),
        }
    }

    /// Notes what the policy decided, for the lines written from now on.
    pub(crate) fn with_policy(mut self, decision: &'static str, reason: Option<&str>) -> Self {
        self.record.policy_decision = Some(decision);
        self.record.policy_reason = reason.map(str::to_owned);
        self
    }

    /// Notes the session the request is about, once it has one.
    pub(crate) fn with_session(mut self, session_id: &str, user: &str) -> Self {
        self.record.session_id = session_id.to_owned();
        self.record.user = user.to_owned();
        self
    }

    /// Written before a person is asked, so a request that waits is traceable even if Warp stops
    /// while it does.
    pub(crate) fn record_approval_requested(&self) -> Result<(), AgentBridgeError> {
        self.append(AuditOutcome::ApprovalRequested, |err| {
            format!(
                "The audit log cannot be written, so the request was not queued for approval: {err}"
            )
        })
    }

    /// Written before anything is opened: without a trace, nothing is.
    pub(crate) fn record_start(&self) -> Result<(), AgentBridgeError> {
        self.append(AuditOutcome::Started, |err| {
            format!("The audit log cannot be written, so the request was not run: {err}")
        })
    }

    /// Writes the closing line. Failing to write it is logged and does not fail the request, which
    /// has already run.
    pub(crate) fn finish(mut self, outcome: Result<(), &AgentBridgeError>) {
        let Some(dir) = self.dir.take() else {
            return;
        };
        self.record.duration_ms =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Err(error) = outcome {
            self.record.result = AuditOutcome::Error;
            self.record.error_code = Some(
                ::local_control::ControlError::from(error.clone())
                    .code
                    .to_string(),
            );
            if let AgentBridgeError::PolicyDenied(reason) = error {
                self.record
                    .policy_reason
                    .get_or_insert_with(|| reason.clone());
            }
        }
        if let Err(error) = audit::append(&dir, &self.record) {
            log::warn!("The audit log of a session request could not be written: {error}");
        }
    }

    fn append(
        &self,
        result: AuditOutcome,
        message: impl FnOnce(&AgentBridgeError) -> String,
    ) -> Result<(), AgentBridgeError> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        let record = AuditRecord {
            result,
            ..self.record.clone()
        };
        audit::append(dir, &record).map_err(|error| AgentBridgeError::Io(message(&error)))
    }
}

pub(crate) fn access_label(access: Access) -> &'static str {
    match access {
        Access::ReadOnly => "read_only",
        Access::Full => "full",
    }
}

#[cfg(test)]
#[path = "open_audit_tests.rs"]
mod tests;
