//! Append-only local record of every request an agent made against a session, so that what ran
//! as root can be reviewed afterwards. Output of commands and file contents are never recorded.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

use super::AUDIT_LOG_MAX_BYTES;
use super::error::AgentBridgeError;
use crate::warp_sync::paths::create_private_dir_all;

const AUDIT_DIR: &str = ".warp/agent-bridge";
const AUDIT_FILE_NAME: &str = "audit.jsonl";
const ROTATED_FILE_NAME: &str = "audit.jsonl.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuditOutcome {
    Ok,
    Error,
}

/// One line of the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AuditRecord {
    pub ts_unix: u64,
    pub request_id: Uuid,
    /// The calling client, as it named itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub action: &'static str,
    pub session_id: String,
    pub host: String,
    pub user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub result: AuditOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub duration_ms: u64,
    /// Bytes read or written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// Directory of the audit log, or `None` when the user's home directory is unknown.
pub(crate) fn audit_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(AUDIT_DIR))
}

/// Appends `record` to the audit log in `dir`, creating it (private to the user) if needed.
pub(crate) fn append(dir: &Path, record: &AuditRecord) -> Result<(), AgentBridgeError> {
    append_with_limit(dir, record, AUDIT_LOG_MAX_BYTES)
}

fn append_with_limit(
    dir: &Path,
    record: &AuditRecord,
    max_bytes: u64,
) -> Result<(), AgentBridgeError> {
    let mut line = serde_json::to_string(record)
        .map_err(|err| AgentBridgeError::Io(format!("could not encode the audit record: {err}")))?;
    line.push('\n');

    create_private_dir_all(dir).map_err(io_error("create the audit directory"))?;
    let path = dir.join(AUDIT_FILE_NAME);
    rotate_if_full(dir, &path, max_bytes)?;

    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(io_error("open the audit log"))?;
    file.write_all(line.as_bytes())
        .map_err(io_error("write to the audit log"))
}

/// Moves a log of `max_bytes` or more to `audit.jsonl.1`, replacing the previous rotation.
fn rotate_if_full(dir: &Path, path: &Path, max_bytes: u64) -> Result<(), AgentBridgeError> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() < max_bytes {
        return Ok(());
    }
    fs::rename(path, dir.join(ROTATED_FILE_NAME)).map_err(io_error("rotate the audit log"))
}

fn io_error(action: &'static str) -> impl Fn(std::io::Error) -> AgentBridgeError {
    move |err| AgentBridgeError::Io(format!("could not {action}: {err}"))
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
