//! Validation of the paths and directories an agent supplies.

use super::error::AgentBridgeError;

const MAX_PATH_BYTES: usize = 4096;

/// Top-level directories of kernel or runtime state, where a file read is meaningless (sizes are
/// zero) and a write is dangerous.
const PSEUDO_FS_ROOTS: [&str; 4] = ["proc", "sys", "dev", "run"];

fn invalid(reason: &str) -> AgentBridgeError {
    AgentBridgeError::InvalidParams(reason.to_owned())
}

/// Resolves `input` to a normalized absolute path, taking a relative one from `cwd`.
pub(crate) fn normalize_path(input: &str, cwd: Option<&str>) -> Result<String, AgentBridgeError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(invalid("the path is empty"));
    }
    if input.len() > MAX_PATH_BYTES {
        return Err(invalid("the path is too long"));
    }
    if input.contains(['\0', '\n']) {
        return Err(invalid("the path contains a control character"));
    }
    if input.starts_with('~') {
        return Err(invalid("use an absolute path instead of `~`"));
    }
    let joined = if input.starts_with('/') {
        input.to_owned()
    } else {
        let cwd = cwd
            .filter(|cwd| cwd.starts_with('/'))
            .ok_or_else(|| invalid("the path is relative but the session's directory is unknown"))?;
        format!("{cwd}/{input}")
    };

    let mut components = Vec::new();
    for component in joined.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(invalid("`..` is not supported; use an absolute path")),
            component => components.push(component),
        }
    }
    let Some(first) = components.first() else {
        return Err(invalid("the root directory is not a file"));
    };
    if PSEUDO_FS_ROOTS.contains(first) {
        return Err(invalid(
            "/proc, /sys, /dev and /run are not files; use exec (for example `cat`) instead",
        ));
    }
    Ok(format!("/{}", components.join("/")))
}

/// Checks a working directory for a command: absolute and free of control characters.
pub(crate) fn validate_cwd(cwd: &str) -> Result<(), AgentBridgeError> {
    if !cwd.starts_with('/') {
        return Err(invalid("cwd must be an absolute path"));
    }
    if cwd.len() > MAX_PATH_BYTES {
        return Err(invalid("cwd is too long"));
    }
    if cwd.contains(['\0', '\n']) {
        return Err(invalid("cwd contains a control character"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
