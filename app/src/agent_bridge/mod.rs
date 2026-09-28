//! Agent Bridge: lets an external agent run commands and read and write files in a Warpified
//! remote session that the user explicitly attached, with the privileges of that session's shell.

pub(crate) mod approval;
pub(crate) mod approval_dialog;
pub(crate) mod attachments;
pub(crate) mod audit;
pub(crate) mod error;
pub(crate) mod messages;
pub(crate) mod model;
pub(crate) mod operations;
pub(crate) mod ops;
pub(crate) mod path;
pub(crate) mod policy;
pub(crate) mod recent;
pub(crate) mod script;
pub(crate) mod visible;

use std::time::Duration;

pub(crate) use messages::{
    attached_message, revoked_all_message, revoked_message, setup_command, setup_executable,
};

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

pub(crate) const RECENT_OUTPUT_DEFAULT_COUNT: u32 = 3;
pub(crate) const RECENT_OUTPUT_MAX_COUNT: u32 = 10;

/// Cap on the output of each block returned by `remote.output.recent`.
pub(crate) const RECENT_OUTPUT_MAX_BYTES: usize = 16 * 1024;

/// Cap on the output returned by `remote.exec.visible`, as much as `remote.exec` returns for
/// stdout and stderr together.
pub(crate) const VISIBLE_OUTPUT_MAX_BYTES: usize = 32 * 1024;

/// How often a visible command's block is checked, first while it may still be a quick command
/// and then once it has proven slow.
pub(crate) const VISIBLE_POLL_INTERVAL: Duration = Duration::from_millis(200);
pub(crate) const VISIBLE_SLOW_POLL_INTERVAL: Duration = Duration::from_secs(1);
pub(crate) const VISIBLE_FAST_POLL_WINDOW: Duration = Duration::from_secs(10);

/// How long a visible command may take to show up in a block before the request fails.
pub(crate) const VISIBLE_START_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an attached session may go unused before access is withdrawn.
pub(crate) const ATTACH_IDLE_TTL: Duration = Duration::from_secs(30 * 60);

/// Size at which the audit log is rotated.
pub(crate) const AUDIT_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// A command longer than this cannot be reviewed by a person, so the agent-ops policy denies it
/// outright instead of asking for approval.
pub(crate) const APPROVAL_MAX_COMMAND_BYTES: usize = 2 * 1024;
pub(crate) const APPROVAL_MAX_COMMAND_LINES: usize = 20;

/// How many lines of a file's contents are shown in an approval dialog.
pub(crate) const APPROVAL_PREVIEW_LINES: usize = 40;

/// Relative to the user's home directory.
pub(crate) const POLICY_FILE: &str = ".warp/agent-ops/policy.toml";
// Used starting Task 4.2 (the paired-agents store); remove this `allow` there.
#[allow(dead_code)]
pub(crate) const AGENTS_FILE: &str = ".warp/agent-ops/agents.toml";

/// Upper bound on how many approval requests a single session may have waiting at once, so a
/// misbehaving agent cannot flood the queue.
pub(crate) const MAX_PENDING_APPROVALS_PER_SESSION: usize = 8;
