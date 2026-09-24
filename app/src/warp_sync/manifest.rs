use std::collections::BTreeMap;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use super::WarpSyncError;

const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
}

/// Ownership and permissions of one remote entry, recorded because the local mirror cannot hold
/// them (the local user is not root).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryMeta {
    pub kind: EntryKind,
    /// Full permission bits, including setuid, setgid and sticky.
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub uname: String,
    pub gname: String,
    pub mtime: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRecord {
    pub remote_user: String,
    pub at_unix: u64,
}

/// Per-host record of what the local mirror holds. Keys are absolute remote paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    version: u32,
    host_key: String,
    /// Identifies the remote machine this mirror was downloaded from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    machine_id: Option<String>,
    #[serde(default)]
    entries: BTreeMap<String, EntryMeta>,
    #[serde(default)]
    last_sync: BTreeMap<String, SyncRecord>,
}

impl Manifest {
    pub fn new(host_key: &str) -> Self {
        Self {
            version: MANIFEST_VERSION,
            host_key: host_key.to_owned(),
            machine_id: None,
            entries: BTreeMap::new(),
            last_sync: BTreeMap::new(),
        }
    }

    /// Loads the manifest at `path`, or starts an empty one if the file does not exist. An
    /// unreadable or unrecognized file is an error and is left untouched.
    pub fn load_or_default(path: &Path, host_key: &str) -> Result<Self, WarpSyncError> {
        let contents = match fs::read(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::new(host_key)),
            Err(err) => return Err(manifest_error("could not read", path, &err)),
        };
        let manifest: Self = serde_json::from_slice(&contents).map_err(|err| {
            WarpSyncError::Manifest(format!("{} is not valid: {err}", path.display()))
        })?;
        if manifest.version != MANIFEST_VERSION {
            return Err(WarpSyncError::Manifest(format!(
                "{} has unsupported version {}",
                path.display(),
                manifest.version
            )));
        }
        if manifest.host_key != host_key {
            return Err(WarpSyncError::Manifest(format!(
                "{} belongs to another host",
                path.display()
            )));
        }
        Ok(manifest)
    }

    /// Writes the manifest to a temporary file next to `path` and renames it into place, so a
    /// crash never leaves a half-written manifest.
    pub fn save_atomic(&self, path: &Path) -> Result<(), WarpSyncError> {
        let dir = path.parent().ok_or_else(|| {
            WarpSyncError::Manifest(format!("{} has no parent directory", path.display()))
        })?;
        fs::create_dir_all(dir).map_err(|err| manifest_error("could not create", dir, &err))?;

        let mut temp = NamedTempFile::new_in(dir)
            .map_err(|err| manifest_error("could not write in", dir, &err))?;
        serde_json::to_writer_pretty(&mut temp, self)
            .map_err(|err| WarpSyncError::Manifest(format!("could not serialize: {err}")))?;
        temp.persist(path)
            .map_err(|err| manifest_error("could not replace", path, &err.error))?;
        Ok(())
    }

    /// Replaces `root` and everything below it with `entries`.
    pub fn replace_subtree(&mut self, root: &str, entries: BTreeMap<String, EntryMeta>) {
        self.entries.retain(|path, _| !is_within(path, root));
        self.entries.extend(entries);
    }

    /// Adds or overwrites `entries`, leaving every other entry in place.
    pub fn upsert_entries(&mut self, entries: BTreeMap<String, EntryMeta>) {
        self.entries.extend(entries);
    }

    pub fn entries_under(&self, root: &str) -> BTreeMap<String, EntryMeta> {
        self.entries
            .iter()
            .filter(|(path, _)| is_within(path, root))
            .map(|(path, meta)| (path.clone(), meta.clone()))
            .collect()
    }

    pub fn machine_id(&self) -> Option<&str> {
        self.machine_id.as_deref()
    }

    pub fn set_machine_id(&mut self, machine_id: Option<String>) {
        self.machine_id = machine_id;
    }

    pub fn entry(&self, path: &str) -> Option<&EntryMeta> {
        self.entries.get(path)
    }

    /// The closest recorded directory that strictly contains `path`.
    pub fn nearest_dir_ancestor(&self, path: &str) -> Option<&EntryMeta> {
        let mut current = path;
        while let Some((parent, _)) = current.rsplit_once('/') {
            if let Some(meta) = self
                .entries
                .get(parent)
                .filter(|meta| meta.kind == EntryKind::Dir)
            {
                return Some(meta);
            }
            current = parent;
        }
        None
    }

    pub fn record_sync(&mut self, root: &str, record: SyncRecord) {
        self.last_sync.insert(root.to_owned(), record);
    }

    pub fn last_sync(&self, root: &str) -> Option<&SyncRecord> {
        self.last_sync.get(root)
    }
}

/// Whether `path` is `root` or lies below it. Compares whole components, so `/etc/nginx2` is not
/// within `/etc/nginx`.
fn is_within(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path.strip_prefix(root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn manifest_error(action: &str, path: &Path, err: &io::Error) -> WarpSyncError {
    WarpSyncError::Manifest(format!("{action} {}: {err}", path.display()))
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
