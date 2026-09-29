//! A short line diff of one file for the upload dialog, so that the user can see what a save is
//! about to change on the server.

use similar::{ChangeTag, TextDiff};

use super::baseline;
use super::paths::local_path_for;
use super::remote_script::RemoteKind;
use super::transfer::{PreparedUpload, UploadPlacement};

/// Lines of diff shown in the upload dialog; the rest is summed up in one line.
pub const MAX_DIFF_LINES: usize = 40;
const CONTEXT_LINES: usize = 2;

/// A unified diff from `old` to `new`, cut to `max_lines` lines plus one that says how many were
/// left out. Empty when the two are the same.
pub fn single_file_diff(old: &str, new: &str, max_lines: usize) -> Vec<String> {
    let diff = TextDiff::from_lines(old, new);
    let mut lines = Vec::new();
    for hunk in diff
        .unified_diff()
        .context_radius(CONTEXT_LINES)
        .iter_hunks()
    {
        lines.push(hunk.header().to_string());
        lines.extend(hunk.iter_changes().map(|change| {
            let sign = match change.tag() {
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
                ChangeTag::Equal => ' ',
            };
            format!("{sign}{}", change.value().trim_end_matches(['\n', '\r']))
        }));
    }
    if lines.len() > max_lines {
        let hidden = lines.len() - max_lines;
        lines.truncate(max_lines);
        lines.push(format!("… {hidden} more lines"));
    }
    lines
}

/// What uploading `prepared` changes in the one file it replaces, compared with the copy of the
/// last sync. `None` for folders, new paths, binary files and files without a Git baseline.
pub(super) fn upload_diff(prepared: &PreparedUpload) -> Option<Vec<String>> {
    let replaces_one_file = prepared.probe.kind == RemoteKind::File
        && prepared.placement == UploadPlacement::Replace
        && prepared.archive.files == 1;
    if !replaces_one_file {
        return None;
    }
    let host_dir = prepared.mirror_root.join(&prepared.host_key);
    let synced = baseline::synced_content(&host_dir, &prepared.remote_path)?;
    let local = std::fs::read(local_path_for(
        &prepared.mirror_root,
        &prepared.host_key,
        &prepared.remote_path,
    ))
    .ok()?;
    let old = String::from_utf8(synced).ok()?;
    let new = String::from_utf8(local).ok()?;
    Some(single_file_diff(&old, &new, MAX_DIFF_LINES))
}

#[cfg(test)]
#[path = "file_diff_tests.rs"]
mod tests;
