//! The server directory as a model the rest of the app reads: the hosts in memory, and the scan of
//! the SSH configuration that keeps them up to date.

use std::path::{Path, PathBuf};

use warpui::{Entity, ModelContext, SingletonEntity};

use super::WARP_CONF_FILE;
use super::mirror::{self, MirrorApplied, MirrorLink};
use super::model::{Host, MergeReport, merge_discovered};
use super::ssh_config::discover_aliases;
use super::store::{self, HostsError};

/// Why the SSH configuration is being scanned, which decides what the user is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshMode {
    /// Warp just started; only news is worth a message.
    Startup,
    /// The user asked, so they always get an answer.
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostDirectoryEvent {
    /// The list of hosts may have changed.
    Changed,
    /// Something to tell the user.
    Notice(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refreshed {
    hosts: Vec<Host>,
    report: MergeReport,
    /// There were no hosts before this scan, so what it found is the first import.
    first_import: bool,
    warnings: Vec<String>,
    files: Vec<PathBuf>,
}

#[derive(Default)]
pub(crate) struct HostDirectoryModel {
    hosts: Vec<Host>,
    started: bool,
    refreshing: bool,
    /// The files the last scan read, to tell whether the SSH configuration has changed since.
    source_files: Vec<PathBuf>,
}

impl Entity for HostDirectoryModel {
    type Event = HostDirectoryEvent;
}

impl SingletonEntity for HostDirectoryModel {}

impl HostDirectoryModel {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn hosts(&self) -> &[Host] {
        &self.hosts
    }

    /// Takes over the list after a change that was written to disk.
    pub(crate) fn replace_hosts(&mut self, hosts: Vec<Host>, ctx: &mut ModelContext<Self>) {
        self.hosts = hosts;
        ctx.emit(HostDirectoryEvent::Changed);
        ctx.notify();
    }

    /// The alias of the host that `ssh <ssh_host>` reached, if the directory has it.
    pub(crate) fn alias_for_ssh_host(&self, ssh_host: &str) -> Option<String> {
        let alias = mirror::alias_of_ssh_host(ssh_host);
        self.hosts
            .iter()
            .any(|host| host.alias == alias)
            .then(|| alias.to_owned())
    }

    /// Records the mirror that a Warp Sync run on a session of `alias` used, and warns when the
    /// alias now reaches another machine than before.
    pub(crate) fn observe_mirror(
        &mut self,
        alias: String,
        observed: MirrorLink,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        ctx.spawn(
            async move {
                let applied = mirror::apply_observation(&home, &alias, &observed);
                (alias, applied)
            },
            |me, (alias, applied), ctx| match applied {
                Ok(Some(MirrorApplied {
                    hosts,
                    machine_changed,
                })) => {
                    if machine_changed {
                        ctx.emit(HostDirectoryEvent::Notice(format!(
                            "{alias} now reaches a different machine than before; Warp Sync \
                             keeps a separate mirror for it"
                        )));
                    }
                    me.replace_hosts(hosts, ctx);
                }
                Ok(None) => {}
                Err(error) => log::warn!("[Host Directory] could not record the mirror: {error}"),
            },
        );
    }

    /// Scans once when the first window opens.
    pub(crate) fn start(&mut self, ctx: &mut ModelContext<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        self.refresh(RefreshMode::Startup, ctx);
    }

    pub(crate) fn refresh(&mut self, mode: RefreshMode, ctx: &mut ModelContext<Self>) {
        if self.refreshing {
            return;
        }
        let Some(home) = dirs::home_dir() else {
            self.finish(mode, Err("the home directory is not known".to_owned()), ctx);
            return;
        };
        self.refreshing = true;
        ctx.spawn(
            async move { refresh_blocking(&home).map_err(|err| err.to_string()) },
            move |me, result, ctx| {
                me.refreshing = false;
                me.finish(mode, result, ctx);
            },
        );
    }

    fn finish(
        &mut self,
        mode: RefreshMode,
        result: Result<Refreshed, String>,
        ctx: &mut ModelContext<Self>,
    ) {
        if let Some(message) = refresh_message(mode, &result) {
            ctx.emit(HostDirectoryEvent::Notice(message));
        }
        match result {
            Ok(refreshed) => {
                for warning in &refreshed.warnings {
                    log::warn!("[Host Directory] {warning}");
                }
                self.hosts = refreshed.hosts;
                self.source_files = refreshed.files;
                log::info!(
                    "[Host Directory] {} hosts, from {} SSH configuration files",
                    self.hosts.len(),
                    self.source_files.len()
                );
            }
            Err(error) => log::warn!("[Host Directory] {error}"),
        }
        ctx.emit(HostDirectoryEvent::Changed);
        ctx.notify();
    }
}

/// Reads `hosts.toml`, scans the SSH configuration, and writes the merged list back if it
/// changed. Never touches `hosts.toml` when it cannot be read or parsed.
pub(crate) fn refresh_blocking(home: &Path) -> Result<Refreshed, HostsError> {
    let existing = store::load(home)?;
    let ssh_dir = home.join(".ssh");
    let discovery = discover_aliases(&ssh_dir.join("config"), &ssh_dir, home);
    let (hosts, report) =
        merge_discovered(&existing, &discovery.aliases, &home.join(WARP_CONF_FILE));
    if hosts != existing {
        store::save(home, &hosts)?;
    }
    Ok(Refreshed {
        hosts,
        report,
        first_import: existing.is_empty(),
        warnings: discovery.warnings,
        files: discovery.files,
    })
}

/// What to tell the user about a scan, if anything.
pub(crate) fn refresh_message(
    mode: RefreshMode,
    result: &Result<Refreshed, String>,
) -> Option<String> {
    let refreshed = match result {
        Ok(refreshed) => refreshed,
        Err(error) => return Some(format!("Could not update the server list: {error}")),
    };
    let MergeReport {
        added,
        missing,
        restored,
    } = &refreshed.report;

    let mut parts = Vec::new();
    match (added.len(), refreshed.first_import) {
        (0, _) => {}
        (count, true) => parts.push(format!("Imported {}", ssh_hosts(count))),
        (1, false) => parts.push("Found 1 new SSH host".to_owned()),
        (count, false) => parts.push(format!("Found {count} new SSH hosts")),
    }
    if mode == RefreshMode::Manual {
        if !missing.is_empty() {
            parts.push(format!(
                "{} no longer in the SSH config",
                hosts(missing.len())
            ));
        }
        if !restored.is_empty() {
            parts.push(format!("{} back in the SSH config", hosts(restored.len())));
        }
    }
    if parts.is_empty() {
        return match mode {
            RefreshMode::Startup => None,
            RefreshMode::Manual => Some(format!(
                "No changes; {} in the server list",
                hosts(refreshed.hosts.len())
            )),
        };
    }
    Some(parts.join("; "))
}

fn hosts(count: usize) -> String {
    match count {
        1 => "1 host".to_owned(),
        count => format!("{count} hosts"),
    }
}

fn ssh_hosts(count: usize) -> String {
    match count {
        1 => "1 SSH host".to_owned(),
        count => format!("{count} SSH hosts"),
    }
}

#[cfg(test)]
#[path = "directory_tests.rs"]
mod tests;
