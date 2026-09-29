//! `~/.warp/agent-ops/hosts.toml`: the servers Warp knows about and its metadata for each. It holds
//! no secrets. `parse` and `serialize` are pure; `load` and `save` touch the disk.

use std::io::Write as _;
use std::path::Path;
use std::{fs, io};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use super::HOSTS_FILE;
use super::model::Host;
use crate::warp_sync::paths::create_private_dir_all;

const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostsFile {
    version: u32,
    #[serde(default)]
    hosts: Vec<Host>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum HostsError {
    #[error("could not read {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("{path} is not valid: {reason}")]
    Invalid { path: String, reason: String },
    #[error("could not write {path}: {reason}")]
    Write { path: String, reason: String },
}

/// Parses the contents of `hosts.toml`. An alias may only appear once.
pub(crate) fn parse(text: &str) -> Result<Vec<Host>, String> {
    let file: HostsFile = toml::from_str(text).map_err(|err| err.to_string())?;
    if file.version != FORMAT_VERSION {
        return Err(format!(
            "version {} is not supported (expected {FORMAT_VERSION})",
            file.version
        ));
    }
    for (index, host) in file.hosts.iter().enumerate() {
        if file.hosts[..index]
            .iter()
            .any(|earlier| earlier.alias == host.alias)
        {
            return Err(format!("the host \"{}\" appears twice", host.alias));
        }
    }
    Ok(file.hosts)
}

pub(crate) fn serialize(hosts: &[Host]) -> Result<String, String> {
    let file = HostsFile {
        version: FORMAT_VERSION,
        hosts: hosts.to_vec(),
    };
    toml::to_string_pretty(&file).map_err(|err| err.to_string())
}

/// Loads the hosts from `home`'s `~/.warp/agent-ops/hosts.toml`. A missing file is an empty
/// directory. A file that cannot be read or parsed is an error and is left as it is: it holds
/// things the user typed, so it must never be replaced by a fresh scan.
pub(crate) fn load(home: &Path) -> Result<Vec<Host>, HostsError> {
    let path = home.join(HOSTS_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(HostsError::Read {
                path: path.display().to_string(),
                reason: err.to_string(),
            });
        }
    };
    parse(&text).map_err(|reason| HostsError::Invalid {
        path: path.display().to_string(),
        reason,
    })
}

/// Writes `hosts` atomically: a private temp file in the same directory, then a rename over the
/// real path, so a reader or a crash never sees a half-written file.
pub(crate) fn save(home: &Path, hosts: &[Host]) -> Result<(), HostsError> {
    let path = home.join(HOSTS_FILE);
    let write_error = |reason: String| HostsError::Write {
        path: path.display().to_string(),
        reason,
    };
    let dir = path.parent().unwrap_or(&path);
    create_private_dir_all(dir).map_err(|err| write_error(err.to_string()))?;

    let text = serialize(hosts).map_err(&write_error)?;
    let mut file = NamedTempFile::new_in(dir).map_err(|err| write_error(err.to_string()))?;
    file.write_all(text.as_bytes())
        .map_err(|err| write_error(err.to_string()))?;
    file.persist(&path)
        .map_err(|err| write_error(err.error.to_string()))?;
    Ok(())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
