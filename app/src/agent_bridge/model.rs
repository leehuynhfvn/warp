use futures::channel::oneshot;
use instant::Instant;
use uuid::Uuid;
use warpui::{Entity, ModelContext, SingletonEntity, WindowId};

use super::approval::{ApprovalDecision, ApprovalQueue, ApprovalRequest};
use super::attachments::{Access, AttachmentStatus, Attachments};
use super::error::AgentBridgeError;
use super::operations::{OperationKind, Operations};
use crate::terminal::model::session::SessionId;

/// Which sessions the user has allowed agents to control, and what is waiting for a person to
/// decide on in them. Lives only in memory, so restarting Warp revokes everything and denies
/// every pending request.
#[derive(Default)]
pub struct AgentBridgeModel {
    attachments: Attachments,
    operations: Operations,
    approvals: ApprovalQueue,
}

/// What an observer of [`AgentBridgeModel`] beyond a plain `notify()` may care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBridgeEvent {
    /// A new request needs a person's attention. `window_id` names the window whose toast should
    /// offer to review it.
    ApprovalRequested {
        request_id: Uuid,
        window_id: WindowId,
    },
    /// The queue changed in some other way (decided, timed out, revoked). An observer with a
    /// dialog open on a request should check whether that request is still there.
    ApprovalsChanged,
}

impl Entity for AgentBridgeModel {
    type Event = AgentBridgeEvent;
}

impl SingletonEntity for AgentBridgeModel {}

impl AgentBridgeModel {
    pub(crate) fn attach(&mut self, id: SessionId, access: Access, ctx: &mut ModelContext<Self>) {
        self.attachments.attach(id, access, Instant::now());
        ctx.notify();
    }

    /// Detaches `id` and denies whatever of its requests were still waiting for a decision (mục
    /// 2.4 of the O2 plan: detach is a kill switch for the queue too).
    pub(crate) fn detach(&mut self, id: SessionId, ctx: &mut ModelContext<Self>) -> bool {
        let was_attached = self.attachments.detach(id);
        let revoked = self.approvals.revoke_session(id);
        if was_attached || revoked > 0 {
            ctx.notify();
        }
        if revoked > 0 {
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        was_attached
    }

    pub(crate) fn detach_all(&mut self, ctx: &mut ModelContext<Self>) -> usize {
        let count = self.attachments.detach_all();
        let revoked = self.approvals.revoke_all();
        if count > 0 || revoked > 0 {
            ctx.notify();
        }
        if revoked > 0 {
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        count
    }

    /// Checks that `id` is attached with at least `needed` access. `user` and `host` name the
    /// session in the error when it is not attached.
    pub(crate) fn check(
        &mut self,
        id: SessionId,
        needed: Access,
        user: &str,
        host: &str,
    ) -> Result<(), AgentBridgeError> {
        self.attachments
            .check(id, needed, user, host, Instant::now())
    }

    pub(crate) fn record_use(&mut self, id: SessionId, is_exec: bool) {
        self.attachments.record_use(id, is_exec, Instant::now());
    }

    pub(crate) fn begin_operation(
        &mut self,
        id: SessionId,
        kind: OperationKind,
    ) -> Result<(), AgentBridgeError> {
        self.operations.begin(id, kind)
    }

    pub(crate) fn end_operation(&mut self, id: SessionId, kind: OperationKind) {
        self.operations.end(id, kind);
    }

    pub(crate) fn status(&self, id: SessionId) -> Option<AttachmentStatus> {
        self.attachments.status(id, Instant::now())
    }

    /// Queues `request` for a person to decide on. Emits [`AgentBridgeEvent::ApprovalRequested`]
    /// so a toast/header can offer to review it.
    pub(crate) fn push_approval(
        &mut self,
        request: ApprovalRequest,
        ctx: &mut ModelContext<Self>,
    ) -> Result<oneshot::Receiver<ApprovalDecision>, AgentBridgeError> {
        let request_id = request.request_id;
        let window_id = request.window_id;
        let receiver = self.approvals.push(request)?;
        ctx.notify();
        ctx.emit(AgentBridgeEvent::ApprovalRequested {
            request_id,
            window_id,
        });
        Ok(receiver)
    }

    /// Sends `decision` to whoever is waiting on `request_id`. `false` if it was no longer in the
    /// queue (already decided, or its wait already timed out).
    // Used starting Task 3.3 (the approval dialog's Approve/Deny buttons); remove this `allow`
    // there.
    #[allow(dead_code)]
    pub(crate) fn decide_approval(
        &mut self,
        request_id: Uuid,
        decision: ApprovalDecision,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let decided = self.approvals.decide(request_id, decision);
        if decided {
            ctx.notify();
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        decided
    }

    /// Drops `request_id` without sending a decision, for cleaning up after a wait that timed out.
    pub(crate) fn remove_approval(
        &mut self,
        request_id: Uuid,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let removed = self.approvals.remove(request_id);
        if removed {
            ctx.notify();
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        removed
    }

    /// Denies every pending request across every session ("Deny all waiting agent requests").
    // Used starting Task 3.5 (the palette entry); remove this `allow` there.
    #[allow(dead_code)]
    pub(crate) fn deny_all_approvals(&mut self, ctx: &mut ModelContext<Self>) -> usize {
        let count = self.approvals.revoke_all();
        if count > 0 {
            ctx.notify();
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        count
    }

    pub(crate) fn approval(&self, request_id: Uuid) -> Option<&ApprovalRequest> {
        self.approvals.get(request_id)
    }

    pub(crate) fn pending_approvals_for_session(&self, id: SessionId) -> usize {
        self.approvals.count_for_session(id)
    }

    pub(crate) fn oldest_approval_for_session(&self, id: SessionId) -> Option<&ApprovalRequest> {
        self.approvals.oldest_for_session(id)
    }

    // Used starting Task 3.5 (the toast); remove this `allow` there.
    #[allow(dead_code)]
    pub(crate) fn oldest_approval_in_window(
        &self,
        window_id: WindowId,
    ) -> Option<&ApprovalRequest> {
        self.approvals.oldest_in_window(window_id)
    }

    /// Records "Allow this command in this session" so `command` (trimmed) runs without asking
    /// again until this attachment ends.
    pub(crate) fn allow_command_in_session(&mut self, id: SessionId, command: &str) {
        self.attachments.allow_command(id, command);
    }

    pub(crate) fn is_command_allowed_in_session(&self, id: SessionId, command: &str) -> bool {
        self.attachments
            .is_command_allowed(id, command, Instant::now())
    }

    /// "Trust this session for the rest of the attachment": every later `Decision::Ask` request of
    /// `id` is allowed without queuing it. Returns `false` (and does not notify) if `id` is not
    /// currently attached.
    pub(crate) fn trust_session(&mut self, id: SessionId, ctx: &mut ModelContext<Self>) -> bool {
        let trusted = self.attachments.trust_session(id);
        if trusted {
            ctx.notify();
        }
        trusted
    }

    pub(crate) fn is_session_trusted(&self, id: SessionId) -> bool {
        self.attachments.is_session_trusted(id, Instant::now())
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
