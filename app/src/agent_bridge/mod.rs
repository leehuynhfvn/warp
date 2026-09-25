//! Agent Bridge: lets an external agent run commands and read and write files in a Warpified
//! remote session that the user explicitly attached, with the privileges of that session's shell.

pub(crate) mod error;

use std::time::Duration;

/// Upper bound on the length of a command. The wrapped script is typed through the remote
/// shell's line editor.
pub(crate) const MAX_COMMAND_BYTES: usize = 8 * 1024;

pub(crate) const EXEC_DEFAULT_TIMEOUT_SECS: u32 = 120;

/// In-band commands only run while the shell is idle, so a command cannot be allowed to hold the
/// session for long.
pub(crate) const EXEC_MAX_TIMEOUT_SECS: u32 = 600;

/// How much longer than the remote `timeout` this side waits for the result.
pub(crate) const EXEC_TIMEOUT_GRACE: Duration = Duration::from_secs(15);

/// Output limits keep a tool result below the token limit of MCP clients.
pub(crate) const EXEC_STDOUT_MAX_BYTES: usize = 24 * 1024;
pub(crate) const EXEC_STDERR_MAX_BYTES: usize = 8 * 1024;

/// File contents travel base64-encoded and hex-encoded through the PTY, so larger files should be
/// inspected with `tail` or `grep` instead.
pub(crate) const READ_MAX_FILE_BYTES: usize = 512 * 1024;
pub(crate) const WRITE_MAX_BYTES: usize = 512 * 1024;

/// How long an attached session may go unused before access is withdrawn.
pub(crate) const ATTACH_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// Size at which the audit log is rotated.
pub(crate) const AUDIT_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
