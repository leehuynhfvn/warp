//! `agent.*` actions: manage paired agent clients (mục 3.11 of the O2 agent-ops policy plan).

use ::local_control::{ControlError, ErrorCode, RequestEnvelope};
use warpui::ModelContext;

use crate::local_control::LocalControlBridge;

/// Pairs the calling agent client with Warp, asking the user to approve it the first time.
// Placeholder until Task 4.4 wires this to the approval queue and the paired-agents store
// (`super::super::agent_bridge::pairing`); `agent.pair` is `ActionImplementationStatus::Stub`
// until then, so `validate_request_authority` rejects every call before this is ever reached.
pub(crate) fn pair(
    _request: &RequestEnvelope,
    _ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<serde_json::Value, ControlError> {
    Err(ControlError::new(
        ErrorCode::UnsupportedAction,
        "agent.pair is not implemented yet",
    ))
}
