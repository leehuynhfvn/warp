use ::local_control::{ControlError, ErrorCode};
use thiserror::Error;

use crate::warp_sync::{COMMAND_TIMEOUT, WarpSyncError, printable};

/// Failure of an Agent Bridge operation. The `Display` output is shown to the calling agent and
/// the user verbatim, so it says what to do next.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub(crate) enum AgentBridgeError {
    #[error("The target is not a remote (SSH) session, so agents cannot control it")]
    NotRemoteSession,
    #[error("PowerShell sessions are not supported yet")]
    UnsupportedShell,
    #[error(
        "The session {user}@{host} is not attached. Ask the user to run 'Agent Bridge: Allow \
         agents to control this session' from the Warp command palette in that pane."
    )]
    NotAttached { user: String, host: String },
    #[error(
        "Access to this session expired after a period of inactivity. Ask the user to run \
         'Agent Bridge: Allow agents to control this session' again."
    )]
    AttachmentExpired,
    #[error(
        "This session is attached read-only, so commands and writes are not allowed. Ask the \
         user to run 'Agent Bridge: Allow agents to control this session' to grant full access."
    )]
    ReadOnlyAttachment,
    #[error(
        "The session's shell is busy: the user is running a command, or the command was \
         cancelled. Try again shortly."
    )]
    SessionBusy,
    #[error("The operation did not finish within {secs} seconds")]
    Timeout { secs: u64 },
    #[error("Invalid parameters: {0}")]
    InvalidParams(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    RemoteFailed(String),
    #[error("Could not run a command in the session: {0}")]
    Executor(String),
    #[error("The server returned unexpected output: {0}")]
    UnexpectedOutput(String),
    #[error("Local error: {0}")]
    Io(String),
}

impl From<AgentBridgeError> for ControlError {
    /// Some messages quote what the server printed, so the text is made safe to show in a
    /// terminal.
    fn from(error: AgentBridgeError) -> Self {
        let code = match &error {
            AgentBridgeError::NotRemoteSession | AgentBridgeError::UnsupportedShell => {
                ErrorCode::InvalidSelector
            }
            AgentBridgeError::NotAttached { .. } | AgentBridgeError::AttachmentExpired => {
                ErrorCode::SessionNotAttached
            }
            AgentBridgeError::ReadOnlyAttachment => ErrorCode::InsufficientPermissions,
            AgentBridgeError::SessionBusy => ErrorCode::SessionBusy,
            AgentBridgeError::Timeout { .. } => ErrorCode::Timeout,
            AgentBridgeError::InvalidParams(_) => ErrorCode::InvalidParams,
            AgentBridgeError::Conflict(_) => ErrorCode::TargetStateConflict,
            AgentBridgeError::RemoteFailed(_)
            | AgentBridgeError::Executor(_)
            | AgentBridgeError::UnexpectedOutput(_)
            | AgentBridgeError::Io(_) => ErrorCode::RemoteOperationFailed,
        };
        ControlError::new(code, printable(&error.to_string()))
    }
}

impl From<WarpSyncError> for AgentBridgeError {
    fn from(error: WarpSyncError) -> Self {
        match error {
            WarpSyncError::NotRemoteSession => Self::NotRemoteSession,
            WarpSyncError::UnsupportedShell => Self::UnsupportedShell,
            WarpSyncError::InvalidPath(message) => Self::InvalidParams(message),
            WarpSyncError::Timeout => Self::Timeout {
                secs: COMMAND_TIMEOUT.as_secs(),
            },
            WarpSyncError::Executor(message) => Self::Executor(message),
            WarpSyncError::LocalIo(message) => Self::Io(message),
            WarpSyncError::NotFound(_)
            | WarpSyncError::PermissionDenied { .. }
            | WarpSyncError::SpecialMode(_)
            | WarpSyncError::TooLarge { .. }
            | WarpSyncError::MissingTool(_)
            | WarpSyncError::RemoteCommandFailed { .. }
            | WarpSyncError::CorruptArchive(_)
            | WarpSyncError::UnexpectedArchiveEntry(_)
            | WarpSyncError::NotMirrored(_)
            | WarpSyncError::Manifest(_)
            | WarpSyncError::AlreadyInProgress
            | WarpSyncError::NoEditor
            | WarpSyncError::Editor(_)
            | WarpSyncError::Baseline(_)
            | WarpSyncError::PendingNotFound
            | WarpSyncError::NoSession(_)
            | WarpSyncError::TooManyPending
            | WarpSyncError::AmbiguousSession(_) => Self::RemoteFailed(error.to_string()),
        }
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
