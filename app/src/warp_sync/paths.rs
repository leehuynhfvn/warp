use std::path::{Path, PathBuf};
use std::{fs, io};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::WarpSyncError;

const MAX_REMOTE_PATH_LEN: usize = 4096;
const UNKNOWN_HOST_KEY: &str = "unknown-host";
const STATE_DIR_NAME: &str = ".warp-sync";
const STAGING_DIR_NAME: &str = "staging";
const RECOVERY_DIR_NAME: &str = "recovered";
const HOST_KEY_HASH_BYTES: usize = 4;

/// Top-level directories that hold kernel or runtime state rather than files worth mirroring.
const PSEUDO_FS_ROOTS: [&str; 4] = ["proc", "sys", "dev", "run"];

/// Directory under which every host's mirror lives.
pub fn mirror_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".warp").join("mirrors"))
}

/// Turns a hostname into a single safe directory name. The hostname is reported by the remote
/// host, so a name that had to be altered gets a hash suffix; otherwise a host could pick a name
/// that sanitizes to the directory of another one.
pub fn host_key(hostname: &str) -> String {
    let sanitized: String = hostname
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
    if !sanitized.is_empty() && sanitized == hostname && !sanitized.starts_with('.') {
        return sanitized;
    }
    let base = sanitized.trim_start_matches('.');
    let base = if base.is_empty() {
        UNKNOWN_HOST_KEY
    } else {
        base
    };
    let digest = hex::encode(&Sha256::digest(hostname.as_bytes())[..HOST_KEY_HASH_BYTES]);
    format!("{base}-{digest}")
}

/// Directory holding the mirror of everything synced from `hostname`.
pub fn host_mirror_dir(hostname: &str) -> Option<PathBuf> {
    mirror_root().map(|root| root.join(host_key(hostname)))
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

/// Interprets text selected in a terminal block as a remote path.
pub fn selection_to_remote_path(
    selection: Option<&str>,
    pwd: Option<&str>,
) -> Result<String, WarpSyncError> {
    let selection = selection.map(str::trim).unwrap_or_default();
    if selection.is_empty() {
        return Err(invalid("select the path to sync first"));
    }
    if selection.contains('\n') {
        return Err(invalid("select a single line"));
    }
    normalize_remote_path(selection, pwd)
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

/// A fresh directory for keeping a previous mirror that could not be restored in place.
pub fn recovery_dir(mirror_root: &Path) -> PathBuf {
    mirror_root
        .join(STATE_DIR_NAME)
        .join(RECOVERY_DIR_NAME)
        .join(Uuid::new_v4().to_string())
}

/// Creates `path` and its missing parents so that only the current user can enter them: mirrors
/// hold copies of files that may be readable by root only.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn invalid(reason: &str) -> WarpSyncError {
    WarpSyncError::InvalidPath(reason.to_owned())
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
