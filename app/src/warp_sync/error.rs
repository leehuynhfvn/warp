use thiserror::Error;

/// Failure of a Warp Sync operation. The `Display` output is shown to the user verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WarpSyncError {
    #[error("Warp Sync only works in a remote (SSH) session")]
    NotRemoteSession,
    #[error("Warp Sync does not support PowerShell sessions yet")]
    UnsupportedShell,
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    #[error("Not found on the remote host: {0}")]
    NotFound(String),
    #[error(
        "Permission denied (running as {user}). Run `sudo -i` and Warpify the subshell, then retry."
    )]
    PermissionDenied { user: String },
    #[error("{0} has setuid or setgid permission bits, which Warp Sync will not upload")]
    SpecialMode(String),
    #[error("Too large: {limit_desc}")]
    TooLarge { limit_desc: String },
    #[error("The remote host is missing the required `{0}` tool")]
    MissingTool(&'static str),
    #[error("The remote command timed out")]
    Timeout,
    #[error("Could not run a command in the session: {0}")]
    Executor(String),
    #[error("{}", format_remote_failure(*.exit_code, .message))]
    RemoteCommandFailed {
        exit_code: Option<i32>,
        message: String,
    },
    #[error("The downloaded archive is corrupt: {0}")]
    CorruptArchive(String),
    #[error("The downloaded archive contains an unexpected entry: {0}")]
    UnexpectedArchiveEntry(String),
    #[error("Nothing to upload: {0} has not been downloaded to the local mirror")]
    NotMirrored(String),
    #[error("Sync manifest error: {0}")]
    Manifest(String),
    #[error("Local file error: {0}")]
    LocalIo(String),
    #[error("A sync for this path is already in progress")]
    AlreadyInProgress,
    #[error(
        "No editor to open the mirror with: choose VS Code, VS Code Insiders, Cursor or Windsurf \
         under \"Choose an editor to open file links\" in Settings > Code > Editor and Code \
         Review"
    )]
    NoEditor,
    #[error("Could not open the editor: {0}")]
    Editor(String),
    #[error("Could not record the Git baseline: {0}")]
    Baseline(String),
    #[error(
        "That confirmation is no longer pending: it was already answered, cancelled or expired"
    )]
    PendingNotFound,
    #[error("No open Warp session is connected to {0}. Open a session to it in Warp and retry.")]
    NoSession(String),
    #[error("Too many operations are waiting for confirmation. Confirm or cancel some first.")]
    TooManyPending,
    #[error("{0}")]
    AmbiguousSession(String),
}

fn format_remote_failure(exit_code: Option<i32>, message: &str) -> String {
    match (exit_code, message.is_empty()) {
        (Some(code), false) => format!("Remote command failed (exit {code}): {message}"),
        (Some(code), true) => format!("Remote command failed (exit {code})"),
        (None, false) => format!("Remote command failed: {message}"),
        (None, true) => "Remote command failed".to_owned(),
    }
}
