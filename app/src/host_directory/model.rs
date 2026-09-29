//! What Warp knows about one server, and how a fresh scan of the SSH configuration is merged into
//! what it already knew.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_ALIAS_LEN: usize = 64;
const MAX_TAG_LEN: usize = 32;
const MAX_TAGS: usize = 16;

/// Where a host is defined, which decides who may change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HostSource {
    /// A block the user wrote in their own SSH configuration. Warp only reads it.
    SshConfig,
    /// A block Warp wrote to its own file inside the SSH configuration.
    Warp,
}

/// How the user signs in over SSH. A note for the user at this stage; Warp does not act on it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthMethod {
    Key,
    Agent,
    Password,
    #[default]
    Unknown,
}

/// How to get from the SSH login to a root shell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RootLogin {
    /// The SSH login already is root.
    Root,
    SudoNopasswd,
    SudoPassword,
    #[default]
    None,
}

/// Which channel runs commands on the server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Transport {
    #[default]
    InBand,
    Direct,
}

/// One server. The connection details (host name, user, port, jump host, key) are deliberately not
/// stored: OpenSSH resolves them from the SSH configuration, so they cannot drift from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Host {
    pub(crate) alias: String,
    pub(crate) source: HostSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) auth: AuthMethod,
    #[serde(default)]
    pub(crate) root_login: RootLogin,
    #[serde(default)]
    pub(crate) requiretty: bool,
    #[serde(default)]
    pub(crate) transport: Transport,
    /// The alias is no longer in the SSH configuration; the metadata is kept until the user
    /// removes the host.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) missing: bool,
    /// Directory name of this server's Warp Sync mirror.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mirror_key: Option<String>,
    /// `/etc/machine-id` of the server the mirror was downloaded from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) machine_id: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Host {
    pub(crate) fn new(alias: impl Into<String>, source: HostSource) -> Self {
        Self {
            alias: alias.into(),
            source,
            tags: Vec::new(),
            auth: AuthMethod::default(),
            root_login: RootLogin::default(),
            requiretty: false,
            transport: Transport::default(),
            missing: false,
            mirror_key: None,
            machine_id: None,
        }
    }
}

/// Why an alias or a tag was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NameError {
    #[error("is empty")]
    Empty,
    #[error("is too long")]
    TooLong,
    #[error("must start with a letter or a digit")]
    BadStart,
    #[error("may only contain letters, digits, '.', '_' and '-'")]
    BadCharacter,
}

fn validate_name(name: &str, max_len: usize) -> Result<(), NameError> {
    let Some(first) = name.chars().next() else {
        return Err(NameError::Empty);
    };
    if name.len() > max_len {
        return Err(NameError::TooLong);
    }
    if !first.is_ascii_alphanumeric() {
        return Err(NameError::BadStart);
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(NameError::BadCharacter);
    }
    Ok(())
}

/// An alias ends up in `ssh <alias>`, typed into the user's own shell without quoting, so it must
/// not be able to start an option (`-oProxyCommand=…`) or contain anything a shell would act on.
pub(crate) fn validate_alias(alias: &str) -> Result<(), NameError> {
    validate_name(alias, MAX_ALIAS_LEN)
}

/// A tag is written into a comment of the SSH configuration, so it stays a single plain word.
pub(crate) fn validate_tag(tag: &str) -> Result<(), NameError> {
    validate_name(tag, MAX_TAG_LEN)
}

/// The tags in `input`, separated by commas or white space: lowercased, without repeats, in the
/// order given.
pub(crate) fn parse_tags(input: &str) -> Result<Vec<String>, String> {
    let mut tags: Vec<String> = Vec::new();
    for word in input.split(|c: char| c == ',' || c.is_whitespace()) {
        if word.is_empty() {
            continue;
        }
        validate_tag(word).map_err(|reason| format!("The tag \"{word}\" {reason}"))?;
        let tag = word.to_ascii_lowercase();
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    if tags.len() > MAX_TAGS {
        return Err(format!("A host can have at most {MAX_TAGS} tags"));
    }
    Ok(tags)
}

/// A concrete alias found in the SSH configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredHost {
    pub(crate) alias: String,
    /// The file that defines the alias.
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
}

/// What a merge changed, for the message shown to the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MergeReport {
    pub(crate) added: Vec<String>,
    pub(crate) missing: Vec<String>,
    pub(crate) restored: Vec<String>,
}

/// Merges the aliases of a fresh scan into `existing`. The order of `existing` is kept and new
/// hosts are appended. An alias that is defined in `warp_conf`, the file Warp writes, belongs to
/// Warp, which lets a lost `hosts.toml` be rebuilt without turning those hosts into ones Warp
/// cannot edit. A host that Warp created is never marked missing here, because it is not the scan
/// of the user's own configuration that decides whether it exists.
pub(crate) fn merge_discovered(
    existing: &[Host],
    discovered: &[DiscoveredHost],
    warp_conf: &Path,
) -> (Vec<Host>, MergeReport) {
    let mut report = MergeReport::default();
    let mut hosts: Vec<Host> = existing.to_vec();

    for found in discovered {
        match hosts.iter_mut().find(|host| host.alias == found.alias) {
            Some(host) if host.missing => {
                host.missing = false;
                report.restored.push(found.alias.clone());
            }
            Some(_) => {}
            None => {
                let source = if found.file == warp_conf {
                    HostSource::Warp
                } else {
                    HostSource::SshConfig
                };
                hosts.push(Host::new(found.alias.clone(), source));
                report.added.push(found.alias.clone());
            }
        }
    }

    for host in hosts.iter_mut() {
        let still_defined = discovered.iter().any(|found| found.alias == host.alias);
        if !still_defined && !host.missing && host.source == HostSource::SshConfig {
            host.missing = true;
            report.missing.push(host.alias.clone());
        }
    }

    (hosts, report)
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
