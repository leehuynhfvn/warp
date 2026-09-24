use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::WarpSyncError;

const MAX_REMOTE_PATH_LEN: usize = 4096;
const UNKNOWN_HOST_KEY: &str = "unknown-host";
const STATE_DIR_NAME: &str = ".warp-sync";
const STAGING_DIR_NAME: &str = "staging";

/// Top-level directories that hold kernel or runtime state rather than files worth mirroring.
const PSEUDO_FS_ROOTS: [&str; 4] = ["proc", "sys", "dev", "run"];

/// Directory under which every host's mirror lives.
pub fn mirror_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".warp").join("mirrors"))
}

/// Turns a hostname into a single safe directory name.
pub fn host_key(hostname: &str) -> String {
    let mut key: String = hostname
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    // A leading dot would allow `..` and would collide with the hidden state directory.
    if key.starts_with('.') {
        key.replace_range(..1, "_");
    }
    if key.is_empty() {
        UNKNOWN_HOST_KEY.to_owned()
    } else {
        key
    }
}

/// Resolves user input into a canonical absolute remote path, resolving relative input against
/// `pwd`.
pub fn normalize_remote_path(input: &str, pwd: Option<&str>) -> Result<String, WarpSyncError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(invalid("the path is empty"));
    }
    if input.len() > MAX_REMOTE_PATH_LEN {
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
        let pwd = pwd
            .filter(|pwd| pwd.starts_with('/'))
            .ok_or_else(|| invalid("the working directory of this block is unknown"))?;
        format!("{pwd}/{input}")
    };

    let mut components = Vec::new();
    for component in joined.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err(invalid("`..` is not supported")),
            component => components.push(component),
        }
    }

    let Some(first) = components.first() else {
        return Err(invalid("the root directory cannot be synced"));
    };
    if PSEUDO_FS_ROOTS.contains(first) {
        return Err(invalid("/proc, /sys, /dev and /run cannot be synced"));
    }
    Ok(format!("/{}", components.join("/")))
}

/// Splits a normalized absolute path into its parent directory and final component.
pub fn split_parent_name(remote_abs: &str) -> (String, String) {
    match remote_abs.rsplit_once('/') {
        Some(("", name)) => ("/".to_owned(), name.to_owned()),
        Some((parent, name)) => (parent.to_owned(), name.to_owned()),
        None => ("/".to_owned(), remote_abs.to_owned()),
    }
}

/// Location in the local mirror of a normalized absolute remote path.
pub fn local_path_for(mirror_root: &Path, host_key: &str, remote_abs: &str) -> PathBuf {
    mirror_root
        .join(host_key)
        .join(remote_abs.trim_start_matches('/'))
}

pub fn manifest_path(mirror_root: &Path, host_key: &str) -> PathBuf {
    mirror_root
        .join(STATE_DIR_NAME)
        .join(format!("{host_key}.json"))
}

/// A fresh staging directory, on the same filesystem as the mirror so that it can be renamed into
/// place.
pub fn staging_dir(mirror_root: &Path) -> PathBuf {
    mirror_root
        .join(STATE_DIR_NAME)
        .join(STAGING_DIR_NAME)
        .join(Uuid::new_v4().to_string())
}

fn invalid(reason: &str) -> WarpSyncError {
    WarpSyncError::InvalidPath(reason.to_owned())
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
