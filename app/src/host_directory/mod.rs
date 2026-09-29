//! The server directory: the servers the user works on, imported from `~/.ssh/config` or created
//! in Warp, with the metadata Warp keeps about each (tags, how to become root, its mirror).

mod directory;
mod model;
mod provision;
mod search;
mod ssh_config;
mod ssh_resolve;
mod store;
mod warp_conf;

use std::path::{Path, PathBuf};

pub(crate) use directory::{HostDirectoryEvent, HostDirectoryModel, RefreshMode};
pub(crate) use model::{Host, HostSource, RootLogin, Transport, parse_tags, validate_alias};
pub(crate) use provision::{
    HostEdit, ProvisionError, create_host, include_installed, install_include, remove_host,
    update_host,
};
pub(crate) use search::{search, search_all};
pub(crate) use ssh_resolve::{Resolved, SshResolver, SystemSsh};
pub(crate) use warp_conf::{NewHost, validate as validate_new_host};

use crate::features::FeatureFlag;

/// Relative to the user's home directory.
pub(crate) const HOSTS_FILE: &str = ".warp/agent-ops/hosts.toml";

/// The file Warp owns inside the user's SSH configuration; relative to the user's home directory.
pub(crate) const WARP_CONF_FILE: &str = ".ssh/config.d/warp.conf";

pub(crate) fn warp_conf_path(home: &Path) -> PathBuf {
    home.join(WARP_CONF_FILE)
}

/// Whether the server directory is available.
pub(crate) fn is_enabled() -> bool {
    FeatureFlag::AgentOpsHosts.is_enabled()
}
