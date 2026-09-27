//! The agent requests running in each session. A visible command is typed into the user's shell,
//! which cancels any in-band command running there, so it must not overlap another request.
//! Hidden requests may overlap each other: the in-band executor runs them one after another.

use std::collections::HashMap;

use super::error::AgentBridgeError;
use crate::terminal::model::session::SessionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationKind {
    /// Runs through the in-band executor (`remote.exec`, `remote.file.*`).
    Hidden,
    /// Runs as a block in the user's shell (`remote.exec.visible`).
    Visible,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct InFlight {
    hidden: u32,
    visible: bool,
}

impl InFlight {
    fn is_empty(&self) -> bool {
        self.hidden == 0 && !self.visible
    }
}

/// Kept apart from the attachments: revoking a session does not stop what is already running,
/// and every started operation still has to end.
#[derive(Debug, Default)]
pub(crate) struct Operations {
    by_session: HashMap<SessionId, InFlight>,
}

impl Operations {
    /// Registers an operation in `id`, or fails if it would overlap one it must not.
    pub(crate) fn begin(&mut self, id: SessionId, kind: OperationKind) -> Result<(), AgentBridgeError> {
        let in_flight = self.by_session.get(&id).copied().unwrap_or_default();
        let is_free = match kind {
            OperationKind::Hidden => !in_flight.visible,
            OperationKind::Visible => in_flight.is_empty(),
        };
        if !is_free {
            return Err(AgentBridgeError::OperationRunning);
        }
        let in_flight = self.by_session.entry(id).or_default();
        match kind {
            OperationKind::Hidden => in_flight.hidden = in_flight.hidden.saturating_add(1),
            OperationKind::Visible => in_flight.visible = true,
        }
        Ok(())
    }

    /// Unregisters an operation started with [`Self::begin`].
    pub(crate) fn end(&mut self, id: SessionId, kind: OperationKind) {
        let Some(in_flight) = self.by_session.get_mut(&id) else {
            return;
        };
        match kind {
            OperationKind::Hidden => in_flight.hidden = in_flight.hidden.saturating_sub(1),
            OperationKind::Visible => in_flight.visible = false,
        }
        if in_flight.is_empty() {
            self.by_session.remove(&id);
        }
    }
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;
