//! Which Warp Sync mirror belongs to a server, learned from the syncs that ran on a session
//! opened with its alias.

use std::path::{Path, PathBuf};

use super::model::Host;
use super::store::{self, HostsError};
use crate::warp_sync::is_mirror_key;

/// What a Warp Sync run on a session showed about the machine behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MirrorLink {
    /// Directory name of the mirror the sync used.
    pub(crate) mirror_key: String,
    /// `None` when the machine did not report an id.
    pub(crate) machine_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MirrorUpdate {
    /// The host learns which mirror is its own.
    Set(MirrorLink),
    /// The alias reaches a machine other than the one recorded. The link is what to record from
    /// now on.
    Warn(MirrorLink),
    Keep,
}

/// What to do with a host that has just been seen syncing to `observed`.
pub(crate) fn mirror_link_update(existing: &Host, observed: &MirrorLink) -> MirrorUpdate {
    let known_machine = existing.machine_id.as_deref();
    let machine_changed = match (known_machine, observed.machine_id.as_deref()) {
        (Some(known), Some(seen)) => known != seen,
        (Some(_), None) | (None, Some(_)) | (None, None) => false,
    };
    if machine_changed {
        return MirrorUpdate::Warn(observed.clone());
    }

    // A machine that stops reporting its id must not make Warp forget the id it knew.
    let machine_id = observed
        .machine_id
        .clone()
        .or_else(|| known_machine.map(str::to_owned));
    let link = MirrorLink {
        mirror_key: observed.mirror_key.clone(),
        machine_id,
    };
    if existing.mirror_key.as_deref() == Some(link.mirror_key.as_str())
        && existing.machine_id == link.machine_id
    {
        return MirrorUpdate::Keep;
    }
    MirrorUpdate::Set(link)
}

/// What `apply_observation` changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MirrorApplied {
    pub(crate) hosts: Vec<Host>,
    /// The alias now reaches another machine than before.
    pub(crate) machine_changed: bool,
}

/// Records `observed` for `alias` in `hosts.toml`. `None` when nothing needed to change, including
/// when the alias is not in the directory.
pub(crate) fn apply_observation(
    home: &Path,
    alias: &str,
    observed: &MirrorLink,
) -> Result<Option<MirrorApplied>, HostsError> {
    let mut hosts = store::load(home)?;
    let Some(host) = hosts.iter_mut().find(|host| host.alias == alias) else {
        return Ok(None);
    };
    let (link, machine_changed) = match mirror_link_update(host, observed) {
        MirrorUpdate::Keep => return Ok(None),
        MirrorUpdate::Set(link) => (link, false),
        MirrorUpdate::Warn(link) => (link, true),
    };
    host.mirror_key = Some(link.mirror_key);
    host.machine_id = link.machine_id;
    store::save(home, &hosts)?;
    Ok(Some(MirrorApplied {
        hosts,
        machine_changed,
    }))
}

/// The folder of the host's Warp Sync mirror under `mirror_root`. `mirror_key` comes from a file
/// the user can edit, so a key that Warp Sync could not have made gives no folder.
pub(crate) fn mirror_dir(host: &Host, mirror_root: &Path) -> Option<PathBuf> {
    let key = host
        .mirror_key
        .as_deref()
        .filter(|key| is_mirror_key(key))?;
    Some(mirror_root.join(key))
}

/// The alias of the directory that `ssh <ssh_host>` reaches, when `ssh_host` is what the user typed
/// after `ssh`: the alias itself, or `user@alias`.
pub(crate) fn alias_of_ssh_host(ssh_host: &str) -> &str {
    ssh_host
        .rsplit_once('@')
        .map_or(ssh_host, |(_user, alias)| alias)
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
