//! The sessions the user has allowed agents to control, kept only in memory. An attachment is
//! tied to a `SessionId`, so leaving the shell it was granted in (for example `exit` from
//! `sudo -i`) ends it.

use std::collections::HashMap;
use std::time::Duration;

use instant::Instant;

use super::ATTACH_IDLE_TTL;
use super::error::AgentBridgeError;
use crate::terminal::model::session::SessionId;

/// What an attached session may be used for. Variants are ordered by privilege.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Access {
    /// Reading files and listing sessions.
    ReadOnly,
    /// Everything `ReadOnly` allows, plus running commands and writing files.
    Full,
}

/// How an attachment stands at a moment in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttachmentStatus {
    pub access: Access,
    pub idle: Duration,
    pub expires_in: Duration,
    pub exec_count: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct Attachment {
    access: Access,
    last_used: Instant,
    exec_count: u32,
}

impl Attachment {
    fn status(&self, now: Instant) -> AttachmentStatus {
        let idle = now.saturating_duration_since(self.last_used);
        AttachmentStatus {
            access: self.access,
            idle,
            expires_in: ATTACH_IDLE_TTL.saturating_sub(idle),
            exec_count: self.exec_count,
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_used) > ATTACH_IDLE_TTL
    }
}

#[derive(Debug, Default)]
pub(crate) struct Attachments {
    by_session: HashMap<SessionId, Attachment>,
}

impl Attachments {
    /// Attaches `id`, replacing any earlier attachment of it.
    pub(crate) fn attach(&mut self, id: SessionId, access: Access, now: Instant) {
        self.by_session.insert(
            id,
            Attachment {
                access,
                last_used: now,
                exec_count: 0,
            },
        );
    }

    pub(crate) fn detach(&mut self, id: SessionId) -> bool {
        self.by_session.remove(&id).is_some()
    }

    /// Detaches every session and returns how many there were.
    pub(crate) fn detach_all(&mut self) -> usize {
        let count = self.by_session.len();
        self.by_session.clear();
        count
    }

    /// Checks that `id` is attached with at least `needed` access. An expired attachment is
    /// removed. `user` and `host` name the session in the error for one that is not attached,
    /// which the registry knows nothing about.
    pub(crate) fn check(
        &mut self,
        id: SessionId,
        needed: Access,
        user: &str,
        host: &str,
        now: Instant,
    ) -> Result<(), AgentBridgeError> {
        let Some(attachment) = self.by_session.get(&id) else {
            return Err(AgentBridgeError::NotAttached {
                user: user.to_owned(),
                host: host.to_owned(),
            });
        };
        if attachment.is_expired(now) {
            self.by_session.remove(&id);
            return Err(AgentBridgeError::AttachmentExpired);
        }
        if attachment.access < needed {
            return Err(AgentBridgeError::ReadOnlyAttachment);
        }
        Ok(())
    }

    /// Counts a finished request and restarts the idle timer. Does nothing for a session that was
    /// detached while the request ran.
    pub(crate) fn record_use(&mut self, id: SessionId, is_exec: bool, now: Instant) {
        let Some(attachment) = self.by_session.get_mut(&id) else {
            return;
        };
        attachment.last_used = now;
        if is_exec {
            attachment.exec_count = attachment.exec_count.saturating_add(1);
        }
    }

    /// The attachment of `id`, unless it has expired.
    pub(crate) fn get(&self, id: SessionId, now: Instant) -> Option<&Attachment> {
        self.by_session
            .get(&id)
            .filter(|attachment| !attachment.is_expired(now))
    }

    pub(crate) fn status(&self, id: SessionId, now: Instant) -> Option<AttachmentStatus> {
        self.get(id, now).map(|attachment| attachment.status(now))
    }
}

#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
