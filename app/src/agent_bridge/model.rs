use instant::Instant;
use warpui::{Entity, ModelContext, SingletonEntity};

use super::attachments::{Access, Attachment, Attachments};
use super::error::AgentBridgeError;
use crate::terminal::model::session::SessionId;

/// Which sessions the user has allowed agents to control. Lives only in memory, so restarting
/// Warp revokes everything.
#[derive(Default)]
pub struct AgentBridgeModel {
    attachments: Attachments,
}

impl Entity for AgentBridgeModel {
    type Event = ();
}

impl SingletonEntity for AgentBridgeModel {}

impl AgentBridgeModel {
    pub(crate) fn attach(
        &mut self,
        id: SessionId,
        access: Access,
        user: String,
        host: String,
        ctx: &mut ModelContext<Self>,
    ) {
        self.attachments
            .attach(id, access, user, host, Instant::now());
        ctx.notify();
    }

    pub(crate) fn detach(&mut self, id: SessionId, ctx: &mut ModelContext<Self>) -> bool {
        let was_attached = self.attachments.detach(id);
        if was_attached {
            ctx.notify();
        }
        was_attached
    }

    pub(crate) fn detach_all(&mut self, ctx: &mut ModelContext<Self>) -> usize {
        let count = self.attachments.detach_all();
        if count > 0 {
            ctx.notify();
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
            .map(|_| ())
    }

    pub(crate) fn record_use(&mut self, id: SessionId, is_exec: bool) {
        self.attachments.record_use(id, is_exec, Instant::now());
    }

    pub(crate) fn get(&self, id: SessionId) -> Option<&Attachment> {
        self.attachments.get(id, Instant::now())
    }
}
