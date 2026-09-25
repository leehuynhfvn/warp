use std::path::{Path, PathBuf};
use std::{fs, io};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::WarpSyncError;

const MAX_REMOTE_PATH_LEN: usize = 4096;
const UNKNOWN_HOST_KEY: &str = "unknown-host";
pub(super) const STATE_DIR_NAME: &str = ".warp-sync";
const STAGING_DIR_NAME: &str = "staging";
const RECOVERY_DIR_NAME: &str = "recovered";
const DIFFS_DIR_NAME: &str = "diffs";
const COMPARE_DIR_NAME: &str = "compare";
const MAX_DIFF_STEM_CHARS: usize = 150;
const HOST_KEY_HASH_BYTES: usize = 4;

/// Git metadata is never mirrored: the root of a host's mirror holds the Git baseline, and a
/// server's own repository configuration could make a local `git` run programs.
pub const GIT_DIR_NAME: &str = ".git";
const GIT_DIR_SHORT_NAME: &str = "git~1";

/// Top-level directories that hold kernel or runtime state rather than files worth mirroring.
const PSEUDO_FS_ROOTS: [&str; 4] = ["proc", "sys", "dev", "run"];

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

/// The file that holds the latest comparison of `remote_path` on the host with `host_key`. It
/// lives next to the manifests rather than in the mirror so that it is never mistaken for a
/// synced file.
pub fn diff_path(mirror_root: &Path, host_key: &str, remote_path: &str) -> PathBuf {
    let stem: String = remote_path
        .trim_start_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_DIFF_STEM_CHARS)
        .collect();
    let stem = if stem.is_empty() { "root" } else { &stem };
    mirror_root
        .join(STATE_DIR_NAME)
        .join(DIFFS_DIR_NAME)
        .join(host_key)
        .join(format!("{stem}.diff"))
}

/// Where the server's copy from the latest comparison of each path on the host with `host_key` is
/// kept, laid out like the mirror of that host.
pub fn compare_dir(mirror_root: &Path, host_key: &str) -> PathBuf {
    mirror_root
        .join(STATE_DIR_NAME)
        .join(COMPARE_DIR_NAME)
        .join(host_key)
}

/// Whether some filesystem resolves `name` to `.git`: case-insensitive volumes, NTFS (trailing dots
/// and spaces, alternate data streams, the `GIT~1` short name) and HFS+ (ignored code points) all
/// do for names that differ from it.
pub fn is_git_metadata_name(name: &str) -> bool {
    let without_stream = name.split(':').next().unwrap_or_default();
    let visible: String = without_stream
        .chars()
        .filter(|c| !is_hfs_ignorable(*c))
        .collect();
    let folded = visible.trim_end_matches(['.', ' ']).to_lowercase();
    folded == GIT_DIR_NAME || folded == GIT_DIR_SHORT_NAME
}

/// Code points that HFS+ leaves out when comparing names.
fn is_hfs_ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}'
    )
}

/// `text` with control characters escaped, so that a remote file name cannot add lines to
/// something shown to the user.
pub fn printable(text: &str) -> String {
    let mut printable = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            printable.extend(c.escape_default());
        } else {
            printable.push(c);
        }
    }
    printable
}

/// Directory holding the mirror of everything synced from `hostname`.
pub fn host_mirror_dir(mirror_root: &Path, hostname: &str) -> PathBuf {
    mirror_root.join(host_key(hostname))
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
            component if is_git_metadata_name(component) => {
                return Err(invalid("Git metadata (`.git`) is not synced"));
            }
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

/// The directory name for a mirror of a host whose plain `host_key` is already used by another
/// machine.
pub fn machine_host_key(host_key: &str, machine_id: &str) -> String {
    let digest = hex::encode(&Sha256::digest(machine_id.as_bytes())[..HOST_KEY_HASH_BYTES]);
    format!("{host_key}-{digest}")
}

/// Whether the mirror folder `dir_name` can belong to a machine that reports `hostname`: it is
/// either the hostname's own folder or that folder with a machine suffix. This only narrows down
/// the sessions worth trying: the transfer itself checks which mirror the machine resolves to.
pub fn host_dir_matches(dir_name: &str, hostname: &str) -> bool {
    let key = host_key(hostname);
    if dir_name == key {
        return true;
    }
    dir_name
        .strip_prefix(key.as_str())
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|suffix| {
            suffix.len() == HOST_KEY_HASH_BYTES * 2 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
        })
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
