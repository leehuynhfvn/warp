//! Detects changes made on the remote host since the last sync, so that an upload does not
//! silently overwrite them.

use std::collections::BTreeMap;

use super::archive::UploadArchive;
use super::manifest::{EntryKind, EntryMeta};

/// What an upload would overwrite on the remote host without the user having seen it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteConflicts {
    /// Files whose content on the host differs from what was last synced.
    pub changed: Vec<String>,
    /// Files that were last synced but are gone from the host or could not be read.
    pub missing: Vec<String>,
    /// New local files whose path already exists on the host.
    pub already_exist: Vec<String>,
}

impl RemoteConflicts {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.missing.is_empty() && self.already_exist.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCheck {
    /// The host has no tool to hash files, so nothing could be compared.
    Unavailable,
    Checked(RemoteConflicts),
}

/// Compares the hashes the host reports for its files (`remote`, keyed by absolute path) with
/// what the manifest recorded (`known`) for every file that `archive` will write.
pub fn find_remote_conflicts(
    known: &BTreeMap<String, EntryMeta>,
    archive: &UploadArchive,
    remote: &BTreeMap<String, String>,
) -> RemoteConflicts {
    let mut conflicts = RemoteConflicts::default();
    for (path, meta) in known {
        let Some(recorded) = meta.sha256.as_deref().filter(|_| meta.kind == EntryKind::File)
        else {
            continue;
        };
        if archive.missing_locally.contains(path) {
            continue;
        }
        match remote.get(path) {
            Some(current) if current == recorded => {}
            Some(_) => conflicts.changed.push(path.clone()),
            None => conflicts.missing.push(path.clone()),
        }
    }
    conflicts.already_exist = archive
        .new_files
        .iter()
        .filter(|path| remote.contains_key(*path))
        .cloned()
        .collect();
    conflicts
}

#[cfg(test)]
#[path = "remote_check_tests.rs"]
mod tests;
