//! The approval queue: a write request the agent-ops policy decided to Ask about waits here for a
//! person to Approve, Deny, or let time out. Kept only in RAM, like the attachments it depends on
//! (`super::attachments`) — closing Warp clears every pending request along with them.

use std::time::{Duration, SystemTime};

use futures::channel::oneshot;
use futures::future::Either;
use uuid::Uuid;
use warpui::WindowId;
use warpui::r#async::Timer;

use super::MAX_PENDING_APPROVALS_PER_SESSION;
use super::error::AgentBridgeError;
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
}

/// One write waiting for a person to decide on it. `agent` is the name the client claimed at
/// connect time; pairing (Phase 4) adds a verified identity alongside it.
#[derive(Debug, Clone)]
pub(crate) struct ApprovalRequest {
    pub(crate) request_id: Uuid,
    pub(crate) session: SessionId,
    /// How the session is named to a person, e.g. "root@draff3".
    // Read starting Task 3.2 (the approval dialog's `content()`); remove this `allow` there.
    #[allow(dead_code)]
    pub(crate) session_label: String,
    #[allow(dead_code)]
    pub(crate) agent: Option<String>,
    #[allow(dead_code)]
    pub(crate) subject: ApprovalSubject,
    /// Wall-clock time the request is auto-denied at, shown to the person reviewing it. The
    /// actual timeout is enforced by the `Timer` in [`wait_for_decision`], not by comparing
    /// against this.
    #[allow(dead_code)]
    pub(crate) deadline: SystemTime,
    /// The window whose pane holds the session, so the right window shows a toast for it.
    pub(crate) window_id: WindowId,
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
    /// the queue while a person is away.
    pub(crate) fn push(
        &mut self,
        request: ApprovalRequest,
    ) -> Result<oneshot::Receiver<ApprovalDecision>, AgentBridgeError> {
        if self.count_for_session(request.session) >= MAX_PENDING_APPROVALS_PER_SESSION {
            return Err(AgentBridgeError::PolicyDenied(
                "too many requests are waiting for approval in this session.".to_owned(),
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
        self.revoke_where(|request| request.session == id)
    }

    /// Denies every pending request and removes them. Used by "Revoke all" and the palette's
    /// "Deny all waiting agent requests".
    pub(crate) fn revoke_all(&mut self) -> usize {
        self.revoke_where(|_| true)
    }

    // Used starting Task 3.2 (via `AgentBridgeModel::approval`, the dialog's content); remove this
    // `allow` there.
    #[allow(dead_code)]
    pub(crate) fn get(&self, request_id: Uuid) -> Option<&ApprovalRequest> {
        self.pending
            .iter()
            .find(|(request, _)| request.request_id == request_id)
            .map(|(request, _)| request)
    }

    pub(crate) fn count_for_session(&self, id: SessionId) -> usize {
        self.pending
            .iter()
            .filter(|(request, _)| request.session == id)
            .count()
    }

    /// The longest-waiting request of `id`, if any (FIFO: entries stay in push order).
    // Used starting Task 3.4 (the header's "N waiting"/Review, via
    // `AgentBridgeModel::oldest_approval_for_session`); remove this `allow` there.
    #[allow(dead_code)]
    pub(crate) fn oldest_for_session(&self, id: SessionId) -> Option<&ApprovalRequest> {
        self.pending
            .iter()
            .find(|(request, _)| request.session == id)
            .map(|(request, _)| request)
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
