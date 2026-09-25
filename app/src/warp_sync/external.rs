//! What a local-control client may ask of Warp Sync. The client names a path in the local mirror
//! and, at most, a session; both are untrusted, so they are mapped onto a host and a remote path
//! here before anything is run.

use std::io;
use std::path::{Component, Path, PathBuf};

use warpui::WindowId;

use super::WarpSyncError;
use super::paths::{STATE_DIR_NAME, host_dir_matches, normalize_remote_path};

const MAX_LISTED_SESSIONS: usize = 5;
const CHOOSE_INSIDE_HOST: &str = "choose a file or folder inside a host folder of the mirror";

/// A path of the local mirror, split into the host it belongs to and its place on that host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorPath {
    /// Name of the host's folder in the mirror.
    pub host_dir_name: String,
    /// The path on the server; absent for the host's folder itself.
    pub remote_path: Option<String>,
}

/// Maps `local_path` onto the host folder and the remote path it stands for.
///
/// The client's path is resolved through the file system, so a symlink that leads out of the
/// mirror is refused. A path that does not exist yet is accepted when the deepest folder that does
/// exist is inside a host folder, which is how a new path of a known host is downloaded for the
/// first time.
pub fn resolve_mirror_path(
    mirror_root: &Path,
    local_path: &str,
) -> Result<MirrorPath, WarpSyncError> {
    let path = Path::new(local_path);
    if !path.is_absolute() || local_path.contains('\0') {
        return Err(invalid("the path must be absolute"));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(invalid("the path must not contain `..`"));
    }
    let root = mirror_root
        .canonicalize()
        .map_err(|_| invalid("the mirror folder does not exist yet"))?;
    let (existing, missing) = split_existing(path)?;
    let relative = existing
        .strip_prefix(&root)
        .map_err(|_| invalid("the path is outside the mirror folder"))?;

    if relative.as_os_str().is_empty() {
        return Err(invalid(CHOOSE_INSIDE_HOST));
    }
    let mut components = relative
        .components()
        .map(|component| component_name(&component))
        .chain(missing.iter().map(|name| component_name_of(name)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();
    let Some(host_dir_name) = components.next().filter(|name| name != STATE_DIR_NAME) else {
        return Err(invalid(CHOOSE_INSIDE_HOST));
    };
    let rest: Vec<String> = components.collect();
    if rest.iter().any(|name| has_unsafe_edges(name)) {
        return Err(invalid(
            "a name in the path starts or ends with a space, or contains a control character",
        ));
    }
    let remote_path = if rest.is_empty() {
        None
    } else {
        Some(normalize_remote_path(&format!("/{}", rest.join("/")), None)?)
    };
    Ok(MirrorPath {
        host_dir_name,
        remote_path,
    })
}

/// Canonicalizes the deepest ancestor of `path` that exists and returns it together with the
/// components below it, deepest last.
fn split_existing(path: &Path) -> Result<(PathBuf, Vec<std::ffi::OsString>), WarpSyncError> {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        match current.canonicalize() {
            Ok(existing) => {
                missing.reverse();
                return Ok((existing, missing));
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(current).is_ok() {
                    return Err(invalid("the path goes through a link that leads nowhere"));
                }
                let (Some(name), Some(parent)) = (current.file_name(), current.parent()) else {
                    return Err(invalid("the path does not exist"));
                };
                missing.push(name.to_owned());
                current = parent;
            }
            Err(err) => return Err(invalid(&format!("the path cannot be used: {err}"))),
        }
    }
}

/// Names that Warp Sync would read differently from how the file system does: it trims the path
/// it is given, and control characters cannot be shown faithfully.
fn has_unsafe_edges(name: &str) -> bool {
    name != name.trim() || name.chars().any(char::is_control)
}

fn component_name(component: &Component<'_>) -> Result<String, WarpSyncError> {
    match component {
        Component::Normal(name) => component_name_of(name),
        Component::Prefix(_) | Component::RootDir | Component::CurDir | Component::ParentDir => {
            Err(invalid("the path is not a plain path inside the mirror"))
        }
    }
}

fn component_name_of(name: &std::ffi::OsStr) -> Result<String, WarpSyncError> {
    name.to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("the path is not valid UTF-8"))
}

fn invalid(reason: &str) -> WarpSyncError {
    WarpSyncError::InvalidPath(reason.to_owned())
}

/// A remote session that could carry out a client's request.
#[derive(Debug, Clone)]
pub struct SessionCandidate<S> {
    pub session: S,
    pub session_id: String,
    pub hostname: String,
    pub user: String,
    pub window_id: WindowId,
    pub tab_index: u32,
    /// Whether the session is the active one of its tab.
    pub is_active: bool,
}

/// Picks the session whose host owns the mirror folder `host_dir_name`.
///
/// When several sessions are connected to the host, the active session of the focused window wins
/// if it is unique; otherwise the caller has to say which one is meant.
pub fn choose_session<S>(
    candidates: Vec<SessionCandidate<S>>,
    host_dir_name: &str,
    focused_window: Option<WindowId>,
) -> Result<SessionCandidate<S>, WarpSyncError> {
    let mut matching: Vec<SessionCandidate<S>> = candidates
        .into_iter()
        .filter(|candidate| host_dir_matches(host_dir_name, &candidate.hostname))
        .collect();
    if matching.len() <= 1 {
        return matching
            .pop()
            .ok_or_else(|| WarpSyncError::NoSession(host_dir_name.to_owned()));
    }
    let preferred: Vec<usize> = matching
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.is_active && Some(candidate.window_id) == focused_window)
        .map(|(index, _)| index)
        .collect();
    if let [index] = preferred.as_slice() {
        return Ok(matching.swap_remove(*index));
    }
    Err(WarpSyncError::AmbiguousSession(describe_ambiguity(
        host_dir_name,
        &matching,
    )))
}

fn describe_ambiguity<S>(host_dir_name: &str, matching: &[SessionCandidate<S>]) -> String {
    let listed = matching
        .iter()
        .take(MAX_LISTED_SESSIONS)
        .map(|candidate| {
            format!(
                "{}@{} (session {}, tab {})",
                candidate.user,
                candidate.hostname,
                candidate.session_id,
                candidate.tab_index + 1
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let more = matching.len().saturating_sub(MAX_LISTED_SESSIONS);
    let suffix = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    format!(
        "Several Warp sessions are connected to {host_dir_name}: {listed}{suffix}. Make the one \
         to use the active session of the focused window, or select it explicitly."
    )
}

#[cfg(test)]
#[path = "external_tests.rs"]
mod tests;
