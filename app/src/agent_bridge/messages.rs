//! The text the Command Palette actions show to the user and put on the clipboard.

use std::io;
use std::path::{Path, PathBuf};

use super::ATTACH_IDLE_TTL;
use super::attachments::Access;
use crate::warp_sync::remote_script::posix_quote;

const REVOKE_ACTION_NAME: &str = "Agent Bridge: Revoke access to this session";

pub(crate) fn attached_message(access: Access, user: &str, host: &str) -> String {
    let minutes = ATTACH_IDLE_TTL.as_secs() / 60;
    let what = match access {
        Access::Full => "run commands and edit files",
        Access::ReadOnly => "read any file that user can read (read-only)",
    };
    format!(
        "Agents can now {what} as {user}@{host} in this session. Access ends after {minutes} \
         minutes without use. Revoke it with '{REVOKE_ACTION_NAME}'."
    )
}

pub(crate) fn revoked_message(was_attached: bool) -> &'static str {
    if was_attached {
        "Agents can no longer use this session."
    } else {
        "Agents did not have access to this session."
    }
}

pub(crate) fn revoked_all_message(count: usize) -> String {
    match count {
        0 => "No session had agent access.".to_owned(),
        1 => "Revoked agent access to 1 session.".to_owned(),
        count => format!("Revoked agent access to {count} sessions."),
    }
}

/// Label next to the title of a pane whose session agents may use.
pub(crate) fn indicator_label(access: Access, user: &str) -> String {
    match access {
        Access::Full => format!("Agents · {user}"),
        Access::ReadOnly => format!("Agents · {user} · read-only"),
    }
}

pub(crate) fn revoke_tooltip(user: &str, host: &str) -> String {
    format!("Revoke agent access to {user}@{host}")
}

/// The executable the MCP client should start. Inside an AppImage, `current_exe` is under a mount
/// point that changes every time the app starts, so the AppImage file itself (which the AppImage
/// runtime puts in `APPIMAGE`) is used instead.
pub(crate) fn setup_executable(
    appimage: Option<PathBuf>,
    current_exe: impl FnOnce() -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    match appimage {
        Some(appimage) if !appimage.as_os_str().is_empty() => Ok(appimage),
        Some(_) | None => current_exe(),
    }
}

/// The command that adds the bridge's MCP server to Claude Code.
pub(crate) fn setup_command(executable: &Path) -> String {
    let executable = posix_quote(&executable.to_string_lossy());
    format!("claude mcp add --scope user warp-bridge -- {executable} --warpctrl mcp")
}

#[cfg(test)]
#[path = "messages_tests.rs"]
mod tests;
