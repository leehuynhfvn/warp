//! Compares a fresh copy of a remote path with the local mirror.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use similar::TextDiff;

use super::WarpSyncError;
use super::archive::LocalFile;
use super::manifest::{EntryKind, EntryMeta};
use super::paths::printable;

/// Files larger than this are listed as different but not diffed line by line.
const MAX_DIFFED_FILE_BYTES: u64 = 1024 * 1024;

/// Once the diff text reaches this size, the remaining files are only listed.
const MAX_DIFF_TEXT_BYTES: usize = 8 * 1024 * 1024;

/// Bounds the time spent on one pathological pair of files; the diff is then valid but not
/// minimal.
const DIFF_TIMEOUT: Duration = Duration::from_secs(2);

const CONTEXT_LINES: usize = 3;
const NO_FILE: &str = "/dev/null";

/// How a file differs between the server and the local mirror, judged against what the mirror
/// last synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    ChangedLocally,
    ChangedOnServer,
    ChangedOnBoth,
    /// Different, but nothing was recorded to tell which side moved.
    ChangedUnknown,
    NewOnServer,
    DeletedLocally,
    NewLocally,
    DeletedOnServer,
}

impl FileChange {
    /// Whether both the server and the local mirror have the file.
    pub fn is_on_both_sides(self) -> bool {
        match self {
            Self::ChangedLocally
            | Self::ChangedOnServer
            | Self::ChangedOnBoth
            | Self::ChangedUnknown => true,
            Self::NewOnServer | Self::DeletedLocally | Self::NewLocally | Self::DeletedOnServer => {
                false
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ChangedLocally => "changed locally",
            Self::ChangedOnServer => "changed on the server",
            Self::ChangedOnBoth => "changed on both sides",
            Self::ChangedUnknown => "differs",
            Self::NewOnServer => "new on the server",
            Self::DeletedLocally => "deleted locally",
            Self::NewLocally => "new locally",
            Self::DeletedOnServer => "deleted on the server",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDifference {
    pub remote_path: String,
    pub change: FileChange,
}

#[derive(Debug)]
pub struct Comparison {
    /// Sorted by remote path.
    pub differences: Vec<FileDifference>,
    pub identical_files: usize,
    /// Unified diff of every difference, server side first.
    pub unified_diff: String,
}

/// The two sides of a comparison and what was last synced.
pub struct ComparedTrees<'a> {
    /// The remote path that was compared.
    pub remote_root: &'a str,
    /// Where the fresh copy of `remote_root` is on this machine.
    pub remote_copy: &'a Path,
    /// Every entry of the fresh copy, keyed by remote path.
    pub remote_entries: &'a BTreeMap<String, EntryMeta>,
    pub local_files: &'a BTreeMap<String, LocalFile>,
    /// What the mirror recorded at the last sync.
    pub recorded: &'a BTreeMap<String, EntryMeta>,
}

pub fn compare_trees(trees: &ComparedTrees<'_>) -> Result<Comparison, WarpSyncError> {
    compare_trees_with_limit(trees, MAX_DIFF_TEXT_BYTES)
}

fn compare_trees_with_limit(
    trees: &ComparedTrees<'_>,
    max_diff_text_bytes: usize,
) -> Result<Comparison, WarpSyncError> {
    let remote_hashes: BTreeMap<&str, &str> = trees
        .remote_entries
        .iter()
        .filter(|(_, meta)| meta.kind == EntryKind::File)
        .filter_map(|(path, meta)| Some((path.as_str(), meta.sha256.as_deref()?)))
        .collect();
    let paths: BTreeSet<&str> = remote_hashes
        .keys()
        .copied()
        .chain(trees.local_files.keys().map(String::as_str))
        .collect();

    let mut comparison = Comparison {
        differences: Vec::new(),
        identical_files: 0,
        unified_diff: String::new(),
    };
    let mut diff_text = DiffText::new(max_diff_text_bytes);
    for path in paths {
        let remote = remote_hashes.get(path).copied();
        let local = trees.local_files.get(path);
        let recorded = trees
            .recorded
            .get(path)
            .and_then(|meta| meta.sha256.as_deref());
        let change = match (remote, local) {
            (Some(remote), Some(local)) if remote == local.sha256 => {
                comparison.identical_files += 1;
                continue;
            }
            (Some(remote), Some(local)) => classify_change(recorded, remote, &local.sha256),
            (Some(_), None) if recorded.is_some() => FileChange::DeletedLocally,
            (Some(_), None) => FileChange::NewOnServer,
            (None, Some(_)) if recorded.is_some() => FileChange::DeletedOnServer,
            (None, Some(_)) => FileChange::NewLocally,
            (None, None) => continue,
        };
        let server_copy = remote.map(|_| disk_path(trees.remote_copy, trees.remote_root, path));
        diff_text.push_file(
            path,
            server_copy.as_deref(),
            local.map(|local| local.path.as_path()),
        )?;
        comparison.differences.push(FileDifference {
            remote_path: path.to_owned(),
            change,
        });
    }
    comparison.unified_diff = diff_text.finish();
    Ok(comparison)
}

/// The text of the report written to disk: a summary followed by the diff.
pub fn render_report(remote_root: &str, hostname: &str, comparison: &Comparison) -> String {
    let mut report = format!(
        "# Warp Sync: {hostname}:{remote_root}\n\
         # In the diff below, '-' is the server and '+' is your local mirror.\n\
         # {} file(s) differ, {} are identical.\n",
        comparison.differences.len(),
        comparison.identical_files
    );
    for difference in &comparison.differences {
        report.push_str(&format!(
            "#   {}: {}\n",
            difference.change.label(),
            printable(&difference.remote_path)
        ));
    }
    report.push('\n');
    report.push_str(&comparison.unified_diff);
    report
}

fn classify_change(recorded: Option<&str>, remote: &str, local: &str) -> FileChange {
    let Some(recorded) = recorded else {
        return FileChange::ChangedUnknown;
    };
    match (local != recorded, remote != recorded) {
        (true, true) => FileChange::ChangedOnBoth,
        (true, false) => FileChange::ChangedLocally,
        (false, true) => FileChange::ChangedOnServer,
        (false, false) => FileChange::ChangedUnknown,
    }
}

/// Where the fresh copy keeps `remote_path`, given that `remote_copy` holds `remote_root`.
fn disk_path(remote_copy: &Path, remote_root: &str, remote_path: &str) -> PathBuf {
    match remote_path
        .strip_prefix(remote_root)
        .map(|rest| rest.trim_start_matches('/'))
    {
        Some(relative) if !relative.is_empty() => remote_copy.join(relative),
        Some(_) | None => remote_copy.to_owned(),
    }
}

enum Side {
    Text(String),
    Binary,
    TooLarge,
}

fn read_side(path: Option<&Path>) -> Result<Side, WarpSyncError> {
    let Some(path) = path else {
        return Ok(Side::Text(String::new()));
    };
    let read_error = |err: std::io::Error| {
        WarpSyncError::LocalIo(format!("could not read {}: {err}", path.display()))
    };
    if fs::metadata(path).map_err(read_error)?.len() > MAX_DIFFED_FILE_BYTES {
        return Ok(Side::TooLarge);
    }
    let bytes = fs::read(path).map_err(read_error)?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Side::Text(text),
        Ok(_) | Err(_) => Side::Binary,
    })
}

struct DiffText {
    text: String,
    max_bytes: usize,
    skipped: usize,
}

impl DiffText {
    fn new(max_bytes: usize) -> Self {
        Self {
            text: String::new(),
            max_bytes,
            skipped: 0,
        }
    }

    fn push_file(
        &mut self,
        remote_path: &str,
        server_copy: Option<&Path>,
        mirror_copy: Option<&Path>,
    ) -> Result<(), WarpSyncError> {
        if self.text.len() >= self.max_bytes {
            self.skipped += 1;
            return Ok(());
        }
        let remote_path = printable(remote_path);
        match (read_side(server_copy)?, read_side(mirror_copy)?) {
            (Side::Text(server), Side::Text(mirror)) => {
                let server_label =
                    server_copy.map_or(NO_FILE.to_owned(), |_| format!("server:{remote_path}"));
                let mirror_label =
                    mirror_copy.map_or(NO_FILE.to_owned(), |_| format!("mirror:{remote_path}"));
                let diff = TextDiff::configure()
                    .timeout(DIFF_TIMEOUT)
                    .diff_lines(&server, &mirror)
                    .unified_diff()
                    .context_radius(CONTEXT_LINES)
                    .header(&server_label, &mirror_label)
                    .to_string();
                self.text.push_str(&diff);
            }
            (Side::TooLarge, _) | (_, Side::TooLarge) => {
                self.text
                    .push_str(&format!("File too large to compare: {remote_path}\n"));
            }
            (Side::Binary, _) | (_, Side::Binary) => {
                self.text
                    .push_str(&format!("Binary files differ: {remote_path}\n"));
            }
        }
        Ok(())
    }

    fn finish(mut self) -> String {
        if self.skipped > 0 {
            self.text.push_str(&format!(
                "\n{} more file(s) differ but are not shown because the diff is too long.\n",
                self.skipped
            ));
        }
        self.text
    }
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
