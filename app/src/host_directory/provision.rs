//! Changes to the files on disk: creating and removing hosts in `warp.conf`, editing what Warp
//! knows about a host, and adding the `Include` line to the user's SSH configuration. A change
//! to `warp.conf` is checked with `ssh -G` before it is kept and undone if it does not read back
//! as intended. Nothing here touches a host block the user wrote.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io};

use tempfile::NamedTempFile;

use super::model::{Host, HostSource, RootLogin, Transport};
use super::ssh_config::discover_aliases;
use super::ssh_resolve::{Resolved, SshResolver};
use super::store::{self, HostsError};
use super::warp_conf::{self, NewHost};
use super::warp_conf_path;
use crate::warp_sync::paths::create_private_dir_all;

const SSH_CONFIG_FILE: &str = ".ssh/config";
const BACKUP_SUFFIX: &str = ".bak";
const CONFIG_BACKUP_PREFIX: &str = "config.warp-backup-";
const DEFAULT_SSH_PORT: u16 = 22;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ProvisionError {
    #[error("{0}")]
    Invalid(String),
    #[error("The alias \"{alias}\" is already defined in {place}")]
    AlreadyDefined { alias: String, place: String },
    #[error("{0}")]
    Io(String),
    #[error("The new host does not read back as entered ({0}); nothing was changed")]
    Verify(String),
    #[error(transparent)]
    Hosts(#[from] HostsError),
}

/// What a person may change about a host in Warp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostEdit {
    pub(crate) tags: Vec<String>,
    pub(crate) root_login: RootLogin,
    pub(crate) transport: Transport,
}

/// Creates the host described by `new`. It must not be defined anywhere yet: `warp.conf` is read
/// before the rest of the SSH configuration, so a second definition would silently replace the
/// first. The exception is a host that Warp knows but that has left the SSH configuration, which
/// is taken over with its tags kept.
pub(crate) fn create_host(
    home: &Path,
    new: &NewHost,
    root_login: RootLogin,
    resolver: &impl SshResolver,
) -> Result<Vec<Host>, ProvisionError> {
    warp_conf::validate(new).map_err(ProvisionError::Invalid)?;
    let mut hosts = store::load(home)?;
    let conf_path = warp_conf_path(home);
    let old_text = read_optional(&conf_path)?;

    let taken_over = hosts.iter().position(|host| host.alias == new.alias);
    if let Some(index) = taken_over
        && !hosts[index].missing
    {
        return Err(already_defined(&new.alias, "the server list".to_owned()));
    }
    let ssh_dir = home.join(".ssh");
    let discovery = discover_aliases(&home.join(SSH_CONFIG_FILE), &ssh_dir, home);
    if let Some(found) = discovery
        .aliases
        .iter()
        .find(|found| found.alias == new.alias)
    {
        let place = format!("{}:{}", found.file.display(), found.line);
        return Err(already_defined(&new.alias, place));
    }
    if old_text
        .as_deref()
        .is_some_and(|text| warp_conf::find_block(text, &new.alias).is_some())
    {
        return Err(already_defined(&new.alias, conf_path.display().to_string()));
    }

    let new_text = warp_conf::append_block(
        old_text.as_deref().unwrap_or(""),
        &warp_conf::render_block(new),
    );
    write_conf(&conf_path, &new_text)?;

    let undo = |error: ProvisionError| {
        restore_conf(&conf_path, old_text.as_deref());
        error
    };
    let resolved = resolver
        .resolve(Some(&conf_path), &new.alias)
        .map_err(|reason| undo(ProvisionError::Verify(reason)))?;
    if let Err(reason) = check_reads_back(new, &resolved) {
        return Err(undo(ProvisionError::Verify(reason)));
    }

    let mut host = Host::new(new.alias.clone(), HostSource::Warp);
    host.tags = new.tags.clone();
    host.root_login = root_login;
    match taken_over {
        Some(index) => {
            let previous = &hosts[index];
            host.auth = previous.auth;
            host.requiretty = previous.requiretty;
            host.transport = previous.transport;
            host.mirror_key = previous.mirror_key.clone();
            host.machine_id = previous.machine_id.clone();
            hosts[index] = host;
        }
        None => hosts.push(host),
    }
    store::save(home, &hosts).map_err(|error| undo(error.into()))?;
    Ok(hosts)
}

fn already_defined(alias: &str, place: String) -> ProvisionError {
    ProvisionError::AlreadyDefined {
        alias: alias.to_owned(),
        place,
    }
}

/// Whether `ssh` resolved the host to what the person entered.
fn check_reads_back(new: &NewHost, resolved: &Resolved) -> Result<(), String> {
    if !resolved.hostname.eq_ignore_ascii_case(&new.hostname) {
        return Err(format!("host name {}", resolved.hostname));
    }
    if let Some(user) = &new.user
        && &resolved.user != user
    {
        return Err(format!("user {}", resolved.user));
    }
    let port = new.port.unwrap_or(DEFAULT_SSH_PORT);
    if resolved.port != port {
        return Err(format!("port {}", resolved.port));
    }
    if let Some(identity_file) = &new.identity_file
        && !resolved.identity_files.contains(identity_file)
    {
        return Err("key file".to_owned());
    }
    if new.proxy_jump.is_some() && resolved.proxy_jump != new.proxy_jump {
        return Err("jump host".to_owned());
    }
    Ok(())
}

/// Removes `alias`. A host Warp created loses its block in `warp.conf`; for one from the user's
/// own configuration only what Warp knows about it is dropped, which is rebuilt empty by the next
/// scan while the alias is still there.
pub(crate) fn remove_host(home: &Path, alias: &str) -> Result<Vec<Host>, ProvisionError> {
    let mut hosts = store::load(home)?;
    let Some(index) = hosts.iter().position(|host| host.alias == alias) else {
        return Err(ProvisionError::Invalid(format!(
            "There is no host \"{alias}\""
        )));
    };
    let conf_path = warp_conf_path(home);
    let old_text = read_optional(&conf_path)?;
    if hosts[index].source == HostSource::Warp
        && let Some(text) = old_text.as_deref()
        && let Some(without) = warp_conf::remove_block(text, alias)
    {
        write_conf(&conf_path, &without)?;
    }
    hosts.remove(index);
    store::save(home, &hosts).map_err(|error| {
        restore_conf(&conf_path, old_text.as_deref());
        ProvisionError::from(error)
    })?;
    Ok(hosts)
}

/// Applies `edit` to `alias`. Tags of a host Warp created are also written to its block, which is
/// where they live.
pub(crate) fn update_host(
    home: &Path,
    alias: &str,
    edit: &HostEdit,
) -> Result<Vec<Host>, ProvisionError> {
    let mut hosts = store::load(home)?;
    let Some(host) = hosts.iter_mut().find(|host| host.alias == alias) else {
        return Err(ProvisionError::Invalid(format!(
            "There is no host \"{alias}\""
        )));
    };
    let tags_changed = host.tags != edit.tags;
    host.tags = edit.tags.clone();
    host.root_login = edit.root_login;
    host.transport = edit.transport;
    let is_warp_host = host.source == HostSource::Warp;

    let conf_path = warp_conf_path(home);
    let old_text = read_optional(&conf_path)?;
    if is_warp_host
        && tags_changed
        && let Some(text) = old_text.as_deref()
        && let Some(with_tags) = warp_conf::set_tags(text, alias, &edit.tags)
    {
        write_conf(&conf_path, &with_tags)?;
    }
    store::save(home, &hosts).map_err(|error| {
        restore_conf(&conf_path, old_text.as_deref());
        ProvisionError::from(error)
    })?;
    Ok(hosts)
}

/// Whether the user's SSH configuration reads `warp.conf`.
pub(crate) fn include_installed(home: &Path) -> bool {
    let text = read_optional(&home.join(SSH_CONFIG_FILE))
        .ok()
        .flatten()
        .unwrap_or_default();
    warp_conf::has_warp_include(&text, &warp_conf_path(home), home)
}

/// Adds the `Include` line for `warp.conf` at the top of the user's SSH configuration and returns
/// where the previous version was saved (`None` if there was nothing to save or nothing to do).
pub(crate) fn install_include(home: &Path) -> Result<Option<PathBuf>, ProvisionError> {
    let path = home.join(SSH_CONFIG_FILE);
    // Follow a symlink, so that the link itself is left alone.
    let target = fs::canonicalize(&path).unwrap_or(path);
    let text = read_optional(&target)?;
    if text
        .as_deref()
        .is_some_and(|text| warp_conf::has_warp_include(text, &warp_conf_path(home), home))
    {
        return Ok(None);
    }
    let dir = target
        .parent()
        .ok_or_else(|| ProvisionError::Io("The SSH configuration has no directory".to_owned()))?;

    let backup = match &text {
        Some(text) => {
            let seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            let backup = dir.join(format!("{CONFIG_BACKUP_PREFIX}{seconds}"));
            write_new(&backup, text)?;
            Some(backup)
        }
        None => None,
    };
    let updated = warp_conf::with_warp_include(text.as_deref().unwrap_or(""));
    replace_keeping_mode(&target, &updated)?;
    Ok(backup)
}

fn read_optional(path: &Path) -> Result<Option<String>, ProvisionError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_error("read", path, &err)),
    }
}

fn io_error(action: &str, path: &Path, err: &io::Error) -> ProvisionError {
    ProvisionError::Io(format!("Could not {action} {}: {err}", path.display()))
}

/// Replaces `warp.conf`, keeping the previous version next to it.
fn write_conf(path: &Path, text: &str) -> Result<(), ProvisionError> {
    if path.exists() {
        let mut backup = path.as_os_str().to_owned();
        backup.push(BACKUP_SUFFIX);
        fs::copy(path, PathBuf::from(backup)).map_err(|err| io_error("back up", path, &err))?;
    }
    replace_private(path, text)
}

/// Puts `warp.conf` back as it was, or removes it if it did not exist. A failure is only logged:
/// it happens while another error is being reported.
fn restore_conf(path: &Path, old_text: Option<&str>) {
    let result = match old_text {
        Some(text) => replace_private(path, text).map_err(|err| err.to_string()),
        None => fs::remove_file(path).map_err(|err| err.to_string()),
    };
    if let Err(error) = result {
        log::warn!(
            "[Host Directory] could not restore {}: {error}",
            path.display()
        );
    }
}

/// Writes `text` to `path` through a private temp file in the same directory and a rename, so a
/// reader or a crash never sees half a file.
fn replace_private(path: &Path, text: &str) -> Result<(), ProvisionError> {
    let dir = path
        .parent()
        .ok_or_else(|| ProvisionError::Io(format!("{} has no directory", path.display())))?;
    create_private_dir_all(dir).map_err(|err| io_error("create", dir, &err))?;
    let mut file = NamedTempFile::new_in(dir).map_err(|err| io_error("write", path, &err))?;
    file.write_all(text.as_bytes())
        .map_err(|err| io_error("write", path, &err))?;
    file.persist(path)
        .map_err(|err| io_error("write", path, &err.error))?;
    Ok(())
}

/// Like [`replace_private`], for a file that keeps the permissions it already has.
fn replace_keeping_mode(path: &Path, text: &str) -> Result<(), ProvisionError> {
    let permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
    let dir = path
        .parent()
        .ok_or_else(|| ProvisionError::Io(format!("{} has no directory", path.display())))?;
    let mut file = NamedTempFile::new_in(dir).map_err(|err| io_error("write", path, &err))?;
    file.write_all(text.as_bytes())
        .map_err(|err| io_error("write", path, &err))?;
    if let Some(permissions) = permissions {
        fs::set_permissions(file.path(), permissions)
            .map_err(|err| io_error("write", path, &err))?;
    }
    file.persist(path)
        .map_err(|err| io_error("write", path, &err.error))?;
    Ok(())
}

/// A new private file that must not replace an existing one.
fn write_new(path: &Path, text: &str) -> Result<(), ProvisionError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| io_error("create", path, &err))?;
    file.write_all(text.as_bytes())
        .map_err(|err| io_error("write", path, &err))
}

#[cfg(test)]
#[path = "provision_tests.rs"]
mod tests;
