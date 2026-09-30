//! The approval queue: a write request the agent-ops policy decided to Ask about waits here for a
//! person to Approve, Deny, or let time out. Kept only in RAM, like the attachments it depends on
//! (`super::attachments`) — closing Warp clears every pending request along with them.

use std::time::{Duration, SystemTime};

use futures::channel::oneshot;
use futures::future::Either;
use uuid::Uuid;
use warpui::WindowId;
use warpui::r#async::Timer;

use super::attachments::Access;
use super::error::AgentBridgeError;
use super::opened::ElevationPlan;
use super::{MAX_PENDING_APPROVALS_PER_SESSION, MAX_PENDING_OPEN_APPROVALS_PER_AGENT};
use crate::terminal::model::session::SessionId;

/// What is being asked about. `Write`'s `preview` is already truncated to
/// `APPROVAL_PREVIEW_LINES`, decoded to text when the content is valid UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalSubject {
    Command {
        command: String,
        cwd: Option<String>,
        /// `remote.exec.visible`, typed into the user's shell, versus `remote.exec`, which runs
        /// out of sight.
        visible: bool,
    },
    Write {
        path: String,
        bytes: u64,
        creates: bool,
        preview: String,
        preview_truncated_lines: usize,
    },
    /// `agent.pair`: not tied to any session, so [`ApprovalRequest::session`] is `None` for this
    /// subject (mục 3.11 of the O2 plan).
    Pairing { name: String },
    /// `remote.session.open`: no session exists yet, so [`ApprovalRequest::session`] is `None` for
    /// this subject too.
    OpenSession {
        alias: String,
        access: Access,
        purpose: String,
        tags: Vec<String>,
        /// `user@host:port` as OpenSSH resolves the alias; absent when `ssh -G` did not answer.
        connection: Option<String>,
        /// What will be done about becoming root, worked out before asking.
        elevation: ElevationPlan,
    },
}

/// Who a request claims to be from: the name a client gave itself, and — once pairing resolves the
/// envelope's `agent_token` against `~/.warp/agent-ops/agents.toml` — a verified id alongside it.
/// `claimed` never grants anything on its own (it is not even validated the same way twice: the
/// `remote.*` actions' own `agent` field is checked by `ops::validate_agent`, this is just what an
/// approval dialog or audit line shows); only `agent_id` means the policy actually recognized the
/// caller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AgentLabel {
    pub(crate) claimed: Option<String>,
    pub(crate) agent_id: Option<String>,
}

/// One write waiting for a person to decide on it.
#[derive(Debug, Clone)]
pub(crate) struct ApprovalRequest {
    pub(crate) request_id: Uuid,
    /// `None` for a pairing request ([`ApprovalSubject::Pairing`]), which is not tied to any
    /// session.
    pub(crate) session: Option<SessionId>,
    /// How the session is named to a person, e.g. "root@draff3". Empty for a pairing request.
    pub(crate) session_label: String,
    pub(crate) agent: AgentLabel,
    pub(crate) subject: ApprovalSubject,
    /// Wall-clock time the request is auto-denied at, shown to the person reviewing it. The
    /// actual timeout is enforced by the `Timer` in [`wait_for_decision`], not by comparing
    /// against this.
    pub(crate) deadline: SystemTime,
    /// The window whose pane holds the session, so the right window shows a toast for it.
    pub(crate) window_id: WindowId,
}

/// `object_id` of the toast announcing a pairing request. Only one pairing request can wait at a
/// time, so one id is enough to find and dismiss it.
pub(crate) const PAIRING_TOAST_ID: &str = "agent_ops_pairing_request";

/// `object_id` of the toast announcing requests to open a session. All of them share one toast:
/// it goes away once none is waiting, and the palette reviews them oldest first.
pub(crate) const OPEN_TOAST_ID: &str = "agent_ops_open_request";

impl ApprovalRequest {
    /// The toast text announcing this request.
    pub(crate) fn toast_message(&self) -> String {
        match &self.subject {
            ApprovalSubject::Pairing { name } => format!("'{name}' wants to pair with Warp"),
            ApprovalSubject::OpenSession { alias, .. } => {
                let agent = self
                    .agent
                    .agent_id
                    .as_deref()
                    .or(self.agent.claimed.as_deref())
                    .unwrap_or("An agent");
                format!("'{agent}' wants to open a session to {alias}")
            }
            ApprovalSubject::Command { .. } | ApprovalSubject::Write { .. } => {
                format!("An agent is waiting for approval on {}", self.session_label)
            }
        }
    }

    /// The `object_id` of the toast announcing this request when it stays until the request leaves
    /// the queue, which one with no session must: no pane header shows it as waiting once a
    /// short-lived toast is gone. `None` for a request that has a pane to show it.
    pub(crate) fn persistent_toast_id(&self) -> Option<&'static str> {
        match self.subject {
            ApprovalSubject::Pairing { .. } => Some(PAIRING_TOAST_ID),
            ApprovalSubject::OpenSession { .. } => Some(OPEN_TOAST_ID),
            ApprovalSubject::Command { .. } | ApprovalSubject::Write { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalDecision {
    // Constructed starting Task 3.3 (the approval dialog's Approve/Deny buttons); remove these
    // `allow`s there.
    #[allow(dead_code)]
    Approve,
    /// Like `Approve`, and this exact command runs without asking again for the rest of the
    /// session (`Attachment::allowed_commands`, mục 3.5 of the O2 plan).
    AllowInSession,
    #[allow(dead_code)]
    Deny,
    TimedOut,
    Revoked,
}

/// FIFO queue of requests waiting for a decision. Each entry owns the `oneshot::Sender` half of
/// the channel [`push`](Self::push) hands its caller the `Receiver` half of.
#[derive(Default)]
pub(crate) struct ApprovalQueue {
    pending: Vec<(ApprovalRequest, oneshot::Sender<ApprovalDecision>)>,
}

impl ApprovalQueue {
    /// Queues `request`. Fails once its session already has
    /// `MAX_PENDING_APPROVALS_PER_SESSION` requests waiting, so a misbehaving agent cannot flood
    /// the queue while a person is away. A pairing request (no session) has no such limit here —
    /// the caller enforces its own "one pairing request at a time" rule instead
    /// (`AgentBridgeModel::has_pending_pairing`).
    pub(crate) fn push(
        &mut self,
        request: ApprovalRequest,
    ) -> Result<oneshot::Receiver<ApprovalDecision>, AgentBridgeError> {
        if let Some(session) = request.session
            && self.count_for_session(session) >= MAX_PENDING_APPROVALS_PER_SESSION
        {
            return Err(AgentBridgeError::PolicyDenied(
                "too many requests are waiting for approval in this session.".to_owned(),
            ));
        }
        if let ApprovalSubject::OpenSession { .. } = request.subject
            && let Some(agent_id) = &request.agent.agent_id
            && self.count_open_for_agent(agent_id) >= MAX_PENDING_OPEN_APPROVALS_PER_AGENT
        {
            return Err(AgentBridgeError::PolicyDenied(
                "too many requests to open a session are waiting for the user to decide."
                    .to_owned(),
            ));
        }
        let (sender, receiver) = oneshot::channel();
        self.pending.push((request, sender));
        Ok(receiver)
    }

    /// Sends `decision` to whoever is waiting on `request_id` and removes it from the queue.
    /// Returns `false` if the request is no longer there (already decided, or its wait already
    /// timed out) — the caller should not treat that as an error, just as a decision that arrived
    /// too late to matter.
    // Used starting Task 3.3 (via `AgentBridgeModel::decide_approval`); remove this `allow` there.
    #[allow(dead_code)]
    pub(crate) fn decide(&mut self, request_id: Uuid, decision: ApprovalDecision) -> bool {
        let Some(index) = self.position(request_id) else {
            return false;
        };
        let (_, sender) = self.pending.remove(index);
        // The receiver may already be gone if its wait just timed out and the caller has not yet
        // called `remove` to clean up the entry (the race noted in mục 3.4 of the O2 plan); that
        // is not an error here, since the request did leave the queue.
        let _ = sender.send(decision);
        true
    }

    /// Drops `request_id` from the queue without sending anything, for cleaning up after a wait
    /// that has already timed out. Returns `false` if it was already gone — for example another
    /// decision reached it first (the same race `decide` documents).
    pub(crate) fn remove(&mut self, request_id: Uuid) -> bool {
        let Some(index) = self.position(request_id) else {
            return false;
        };
        self.pending.remove(index);
        true
    }

    /// Denies every request of `id` and removes them. Used when the user detaches the session.
    pub(crate) fn revoke_session(&mut self, id: SessionId) -> usize {
        self.revoke_where(|request| request.session == Some(id))
    }

    /// Denies every pending request and removes them. Used by "Revoke all" and the palette's
    /// "Deny all waiting agent requests".
    pub(crate) fn revoke_all(&mut self) -> usize {
        self.revoke_where(|_| true)
    }

    pub(crate) fn get(&self, request_id: Uuid) -> Option<&ApprovalRequest> {
        self.pending
            .iter()
            .find(|(request, _)| request.request_id == request_id)
            .map(|(request, _)| request)
    }

    pub(crate) fn count_for_session(&self, id: SessionId) -> usize {
        self.pending
            .iter()
            .filter(|(request, _)| request.session == Some(id))
            .count()
    }

    /// The longest-waiting request of `id`, if any (FIFO: entries stay in push order).
    pub(crate) fn oldest_for_session(&self, id: SessionId) -> Option<&ApprovalRequest> {
        self.pending
            .iter()
            .find(|(request, _)| request.session == Some(id))
            .map(|(request, _)| request)
    }

    /// Whether a pairing request ([`ApprovalSubject::Pairing`]) is already waiting — `agent.pair`
    /// allows only one at a time, across the whole app, so a paired-or-not-yet-answered client
    /// cannot spam the pairing dialog (mục 3.11 of the plan).
    pub(crate) fn has_pending_pairing(&self) -> bool {
        self.pending
            .iter()
            .any(|(request, _)| matches!(request.subject, ApprovalSubject::Pairing { .. }))
    }

    /// Whether a request to open a session ([`ApprovalSubject::OpenSession`]) is waiting.
    pub(crate) fn has_pending_open(&self) -> bool {
        self.pending
            .iter()
            .any(|(request, _)| matches!(request.subject, ApprovalSubject::OpenSession { .. }))
    }

    #[cfg(test)]
    pub(crate) fn pending_open_request_ids(&self) -> Vec<Uuid> {
        self.pending
            .iter()
            .filter(|(request, _)| matches!(request.subject, ApprovalSubject::OpenSession { .. }))
            .map(|(request, _)| request.request_id)
            .collect()
    }

    /// How many requests to open a session `agent_id` has waiting.
    pub(crate) fn count_open_for_agent(&self, agent_id: &str) -> usize {
        self.pending
            .iter()
            .filter(|(request, _)| {
                matches!(request.subject, ApprovalSubject::OpenSession { .. })
                    && request.agent.agent_id.as_deref() == Some(agent_id)
            })
            .count()
    }

    /// The longest-waiting request whose session's pane is in `window_id`.
    // Used starting Task 3.5 (the toast, via `AgentBridgeModel::oldest_approval_in_window`);
    // remove this `allow` there.
    #[allow(dead_code)]
    pub(crate) fn oldest_in_window(&self, window_id: WindowId) -> Option<&ApprovalRequest> {
        self.pending
            .iter()
            .find(|(request, _)| request.window_id == window_id)
            .map(|(request, _)| request)
    }

    fn position(&self, request_id: Uuid) -> Option<usize> {
        self.pending
            .iter()
            .position(|(request, _)| request.request_id == request_id)
    }

    /// `Vec::retain` only hands its predicate a shared reference, but sending a decision needs to
    /// consume the `Sender` — so matching entries are partitioned out first, then sent to.
    fn revoke_where(&mut self, mut matches: impl FnMut(&ApprovalRequest) -> bool) -> usize {
        let (revoked, remaining): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(request, _)| matches(request));
        self.pending = remaining;
        let count = revoked.len();
        for (_, sender) in revoked {
            let _ = sender.send(ApprovalDecision::Revoked);
        }
        count
    }
}

/// Waits for a decision on `receiver`, for at most `timeout`. A dropped sender (the request was
/// `remove`d without a decision) is treated the same as an explicit revoke: either way, the
/// request is gone and nothing will run.
pub(crate) async fn wait_for_decision(
    receiver: oneshot::Receiver<ApprovalDecision>,
    timeout: Duration,
) -> ApprovalDecision {
    let timer = Timer::after(timeout);
    futures::pin_mut!(receiver);
    futures::pin_mut!(timer);
    match futures::future::select(receiver, timer).await {
        Either::Left((Ok(decision), _)) => decision,
        Either::Left((Err(_canceled), _)) => ApprovalDecision::Revoked,
        Either::Right(_) => ApprovalDecision::TimedOut,
    }
}

#[cfg(test)]
#[path = "approval_tests.rs"]
mod tests;
