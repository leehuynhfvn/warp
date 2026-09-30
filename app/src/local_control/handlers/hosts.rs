//! `remote.host.list`: the servers of the user's directory, with what Warp knows about each. Like
//! `remote.session.list` it changes nothing and needs no attached session; it only describes.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use ::local_control::protocol::{
    DEFAULT_HOST_LIST_LIMIT, MAX_HOST_LIST_LIMIT, MAX_HOST_QUERY_LEN, MAX_HOST_SYNCED_PATHS,
    RemoteHostListParams, RemoteHostListResult, RemoteHostMirror, RemoteHostSession,
    RemoteHostSource, RemoteHostSummary, RemoteRootLogin, RemoteTransport, RequestEnvelope,
};
use ::local_control::{ActionKind, ControlError, ErrorCode};
use futures::channel::oneshot;
use instant::Instant;
use warpui::{ModelContext, SingletonEntity};

use super::remote::{ListedSession, RemoteReceiver, ensure_enabled, listed_sessions};
use crate::host_directory::{
    self, Host, HostDirectoryModel, HostSource, RootLogin, SshResolver, SystemSsh, Transport,
    alias_of_ssh_host, mirror_dir,
};
use crate::local_control::LocalControlBridge;
use crate::warp_sync::{SyncConfig, printable, synced_paths};

/// `ssh -G` runs `Match exec` commands of the user's own configuration, so a request may not keep
/// running it indefinitely: hosts still unresolved when the budget is spent are listed without
/// their connection.
pub(super) const RESOLVE_BUDGET: Duration = Duration::from_secs(5);

pub(crate) fn host_list(
    request: &RequestEnvelope,
    ctx: &mut ModelContext<LocalControlBridge>,
) -> Result<RemoteReceiver, ControlError> {
    let kind = request.action.kind;
    ensure_enabled(kind)?;
    if !host_directory::is_enabled() {
        return Err(ControlError::new(
            ErrorCode::UnsupportedAction,
            format!(
                "{} requires the server directory, which is not enabled in this build",
                kind.as_str()
            ),
        ));
    }
    let params = request.action.params_as::<RemoteHostListParams>()?;
    let (query, limit) = validate(&params)?;

    let all_hosts = HostDirectoryModel::as_ref(ctx).hosts().to_vec();
    let (hosts, total) = select_hosts(&all_hosts, query, limit);
    let sessions = listed_sessions(ActionKind::RemoteSessionList, ctx)?;
    let mirror_root = match SyncConfig::from_settings(ctx) {
        Ok(config) => Some(config.mirror_root),
        Err(error) => {
            log::warn!("[Host Directory] mirrors are not listed: {error}");
            None
        }
    };

    let (sender, receiver) = oneshot::channel();
    ctx.spawn(
        async move {
            let deadline = Instant::now() + RESOLVE_BUDGET;
            let details = gather_details(&hosts, mirror_root.as_deref(), &SystemSsh, deadline);
            build_result(&hosts, details, &sessions, total)
        },
        move |_, result, _| {
            let value = serde_json::to_value(result).map_err(|err| {
                ControlError::with_details(
                    ErrorCode::Internal,
                    "failed to serialize the host list",
                    err.to_string(),
                )
            });
            if sender.send(value).is_err() {
                log::debug!("A local-control client stopped waiting for the host list");
            }
        },
    );
    Ok(receiver)
}

fn validate(params: &RemoteHostListParams) -> Result<(&str, usize), ControlError> {
    let query = params.query.as_deref().unwrap_or_default();
    if query.len() > MAX_HOST_QUERY_LEN {
        return Err(ControlError::new(
            ErrorCode::InvalidParams,
            format!("query is longer than {MAX_HOST_QUERY_LEN} bytes"),
        ));
    }
    let limit = params.limit.unwrap_or(DEFAULT_HOST_LIST_LIMIT);
    if !(1..=MAX_HOST_LIST_LIMIT).contains(&limit) {
        return Err(ControlError::new(
            ErrorCode::InvalidParams,
            format!("limit must be between 1 and {MAX_HOST_LIST_LIMIT}"),
        ));
    }
    Ok((query, limit as usize))
}

/// The first `limit` hosts that match `query`, and how many matched.
fn select_hosts(hosts: &[Host], query: &str, limit: usize) -> (Vec<Host>, usize) {
    let found = host_directory::search_all(hosts, query);
    let total = found.len();
    (found.into_iter().take(limit).cloned().collect(), total)
}

/// What is read from outside the directory for one host.
#[derive(Debug, Default, PartialEq, Eq)]
struct HostDetails {
    connection: Option<String>,
    mirror: Option<RemoteHostMirror>,
}

/// Runs `ssh -G` and reads manifests, so it must not run on the main thread. Hosts are resolved
/// one after the other until `deadline`.
fn gather_details(
    hosts: &[Host],
    mirror_root: Option<&Path>,
    resolver: &dyn SshResolver,
    deadline: Instant,
) -> Vec<HostDetails> {
    hosts
        .iter()
        .map(|host| HostDetails {
            connection: resolve_connection(host, resolver, deadline),
            mirror: mirror_root.and_then(|root| read_mirror(host, root)),
        })
        .collect()
}

pub(super) fn resolve_connection(
    host: &Host,
    resolver: &dyn SshResolver,
    deadline: Instant,
) -> Option<String> {
    if host.missing || Instant::now() >= deadline {
        return None;
    }
    let resolved = resolver.resolve(None, &host.alias).ok()?;
    let hostname = if resolved.hostname.contains(':') {
        format!("[{}]", resolved.hostname)
    } else {
        resolved.hostname
    };
    Some(printable(&format!(
        "{}@{hostname}:{}",
        resolved.user, resolved.port
    )))
}

fn read_mirror(host: &Host, mirror_root: &Path) -> Option<RemoteHostMirror> {
    let dir = mirror_dir(host, mirror_root).filter(|dir| dir.is_dir())?;
    let key = host.mirror_key.as_deref()?;
    let mut synced = match synced_paths(mirror_root, key) {
        Ok(paths) => paths,
        Err(error) => {
            log::warn!("[Host Directory] could not read the manifest of {key}: {error}");
            Vec::new()
        }
    };
    let synced_paths_truncated = synced.len() > MAX_HOST_SYNCED_PATHS;
    synced.truncate(MAX_HOST_SYNCED_PATHS);
    Some(RemoteHostMirror {
        dir: dir.to_string_lossy().into_owned(),
        synced_paths: synced.iter().map(|path| printable(path)).collect(),
        synced_paths_truncated,
    })
}

fn build_result(
    hosts: &[Host],
    details: Vec<HostDetails>,
    sessions: &[ListedSession],
    total: usize,
) -> RemoteHostListResult {
    let mut by_alias: HashMap<&str, Vec<RemoteHostSession>> = HashMap::new();
    for listed in sessions {
        let Some(ssh_host) = listed.ssh_host.as_deref() else {
            continue;
        };
        by_alias
            .entry(alias_of_ssh_host(ssh_host))
            .or_default()
            .push(RemoteHostSession {
                session_id: listed.summary.session_id.clone(),
                is_active: listed.summary.is_active,
                attached: listed.summary.attached.clone(),
            });
    }
    let hosts = hosts
        .iter()
        .zip(details)
        .map(|(host, details)| summarize(host, details, by_alias.remove(host.alias.as_str())))
        .collect();
    RemoteHostListResult {
        hosts,
        total: u32::try_from(total).unwrap_or(u32::MAX),
    }
}

fn summarize(
    host: &Host,
    details: HostDetails,
    sessions: Option<Vec<RemoteHostSession>>,
) -> RemoteHostSummary {
    RemoteHostSummary {
        alias: host.alias.clone(),
        tags: host.tags.clone(),
        source: match host.source {
            HostSource::SshConfig => RemoteHostSource::SshConfig,
            HostSource::Warp => RemoteHostSource::Warp,
        },
        missing: host.missing,
        connection: details.connection,
        root_login: match host.root_login {
            RootLogin::Root => RemoteRootLogin::Root,
            RootLogin::SudoNopasswd => RemoteRootLogin::SudoNopasswd,
            RootLogin::SudoPassword => RemoteRootLogin::SudoPassword,
            RootLogin::None => RemoteRootLogin::None,
        },
        transport: match host.transport {
            Transport::InBand => RemoteTransport::InBand,
            Transport::Direct => RemoteTransport::Direct,
        },
        sessions: sessions.unwrap_or_default(),
        mirror: details.mirror,
    }
}

#[cfg(test)]
#[path = "hosts_tests.rs"]
mod tests;
