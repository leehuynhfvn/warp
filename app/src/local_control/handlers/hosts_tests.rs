use std::cell::Cell;
use std::fs;
use std::path::Path;

use ::local_control::protocol::{RemoteAccess, RemoteAttachment, RemoteSessionKind};

use super::*;
use crate::host_directory::Resolved;
use crate::warp_sync::paths::manifest_path;

struct FakeSsh {
    calls: Cell<usize>,
}

impl FakeSsh {
    fn new() -> Self {
        Self {
            calls: Cell::new(0),
        }
    }
}

impl SshResolver for FakeSsh {
    fn resolve(&self, _config: Option<&Path>, alias: &str) -> Result<Resolved, String> {
        self.calls.set(self.calls.get() + 1);
        if alias == "broken" {
            return Err("ssh -G failed".to_owned());
        }
        Ok(Resolved {
            hostname: format!("{alias}.example"),
            user: "ops".to_owned(),
            port: 2222,
            proxy_jump: None,
            identity_files: vec!["~/.ssh/id_ed25519".to_owned()],
        })
    }
}

fn host(alias: &str) -> Host {
    Host::new(alias, HostSource::SshConfig)
}

fn far_deadline() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

fn params(query: Option<&str>, limit: Option<u32>) -> RemoteHostListParams {
    RemoteHostListParams {
        query: query.map(str::to_owned),
        limit,
    }
}

fn listed(session_id: &str, ssh_host: Option<&str>, is_active: bool) -> ListedSession {
    ListedSession {
        summary: ::local_control::protocol::RemoteSessionSummary {
            session_id: session_id.to_owned(),
            window_index: 0,
            tab_index: 0,
            pane_index: 0,
            is_active,
            session_type: RemoteSessionKind::Remote,
            host: "web01".to_owned(),
            user: "root".to_owned(),
            shell: "bash".to_owned(),
            cwd: None,
            attached: (session_id == "2").then_some(RemoteAttachment {
                access: RemoteAccess::Full,
                idle_secs: 1,
                expires_in_secs: 60,
                exec_count: 0,
            }),
        },
        ssh_host: ssh_host.map(str::to_owned),
    }
}

/// A mirror folder with a manifest that lists `paths` as synced.
fn write_mirror(root: &Path, key: &str, paths: &[&str]) {
    fs::create_dir_all(root.join(key)).unwrap();
    let last_sync: serde_json::Map<String, serde_json::Value> = paths
        .iter()
        .map(|path| {
            (
                (*path).to_owned(),
                serde_json::json!({ "remote_user": "root", "at_unix": 1 }),
            )
        })
        .collect();
    let manifest = serde_json::json!({ "version": 1, "host_key": key, "last_sync": last_sync });
    write_manifest(root, key, &manifest.to_string());
}

fn write_manifest(root: &Path, key: &str, text: &str) {
    let path = manifest_path(root, key);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn the_default_limit_and_an_empty_query_are_accepted() {
    assert_eq!(validate(&params(None, None)).unwrap(), ("", 50));
}

#[test]
fn a_limit_outside_1_to_100_is_refused() {
    for limit in [0, 101, u32::MAX] {
        let error = validate(&params(None, Some(limit))).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidParams, "{limit}");
    }
    assert_eq!(validate(&params(None, Some(100))).unwrap().1, 100);
    assert_eq!(validate(&params(None, Some(1))).unwrap().1, 1);
}

#[test]
fn a_query_that_is_too_long_is_refused() {
    let long = "a".repeat(MAX_HOST_QUERY_LEN + 1);
    let error = validate(&params(Some(&long), None)).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidParams);
}

#[test]
fn unknown_params_are_refused() {
    let parsed = serde_json::from_value::<RemoteHostListParams>(serde_json::json!({ "all": true }));
    assert!(parsed.is_err());
}

#[test]
fn hosts_are_selected_by_query_and_cut_at_the_limit_with_the_total_kept() {
    let mut prod = host("web02");
    prod.tags = vec!["prod".to_owned()];
    let hosts = [host("web01"), prod, host("db01")];

    let (all, total) = select_hosts(&hosts, "", 2);
    assert_eq!(total, 3);
    assert_eq!(
        all.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(),
        ["db01", "web01"]
    );

    let (tagged, total) = select_hosts(&hosts, "tag:prod", 10);
    assert_eq!((tagged.len(), total), (1, 1));
    assert_eq!(tagged[0].alias, "web02");
}

#[test]
fn hosts_gone_from_the_ssh_config_are_listed_but_not_resolved() {
    let mut gone = host("old");
    gone.missing = true;
    let ssh = FakeSsh::new();

    let details = gather_details(&[host("web01"), gone], None, &ssh, far_deadline());

    assert_eq!(
        details[0].connection.as_deref(),
        Some("ops@web01.example:2222")
    );
    assert_eq!(details[1].connection, None);
    assert_eq!(ssh.calls.get(), 1);
}

#[test]
fn a_host_ssh_cannot_resolve_has_no_connection() {
    let details = gather_details(&[host("broken")], None, &FakeSsh::new(), far_deadline());
    assert_eq!(details, [HostDetails::default()]);
}

#[test]
fn no_host_is_resolved_after_the_deadline() {
    let ssh = FakeSsh::new();
    let details = gather_details(&[host("web01")], None, &ssh, Instant::now());
    assert_eq!(details[0].connection, None);
    assert_eq!(ssh.calls.get(), 0);
}

#[test]
fn an_ipv6_host_name_is_bracketed_so_the_port_can_be_told_apart() {
    struct V6;
    impl SshResolver for V6 {
        fn resolve(&self, _config: Option<&Path>, _alias: &str) -> Result<Resolved, String> {
            Ok(Resolved {
                hostname: "2001:db8::1".to_owned(),
                user: "root".to_owned(),
                port: 22,
                proxy_jump: None,
                identity_files: Vec::new(),
            })
        }
    }
    let connection = resolve_connection(&host("v6"), &V6, far_deadline());
    assert_eq!(connection.as_deref(), Some("root@[2001:db8::1]:22"));
}

#[test]
fn control_characters_in_what_ssh_reports_do_not_reach_the_client() {
    struct Hostile;
    impl SshResolver for Hostile {
        fn resolve(&self, _config: Option<&Path>, _alias: &str) -> Result<Resolved, String> {
            Ok(Resolved {
                hostname: "h\u{1b}[31m".to_owned(),
                user: "u\nroot".to_owned(),
                port: 22,
                proxy_jump: None,
                identity_files: Vec::new(),
            })
        }
    }
    let connection = resolve_connection(&host("x"), &Hostile, far_deadline()).unwrap();
    assert!(!connection.contains(['\u{1b}', '\n']), "{connection:?}");
}

#[test]
fn a_mirror_reports_its_folder_and_synced_paths() {
    let root = tempfile::tempdir().unwrap();
    write_mirror(root.path(), "web01", &["/etc/nginx", "/etc/hosts"]);
    let mut web = host("web01");
    web.mirror_key = Some("web01".to_owned());

    let mirror = read_mirror(&web, root.path()).unwrap();

    assert_eq!(mirror.dir, root.path().join("web01").to_string_lossy());
    assert_eq!(mirror.synced_paths, ["/etc/hosts", "/etc/nginx"]);
    assert!(!mirror.synced_paths_truncated);
}

#[test]
fn synced_paths_are_cut_at_the_limit_and_say_so() {
    let root = tempfile::tempdir().unwrap();
    let paths: Vec<String> = (0..MAX_HOST_SYNCED_PATHS + 5)
        .map(|n| format!("/srv/app/{n:04}"))
        .collect();
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    write_mirror(root.path(), "web01", &refs);
    let mut web = host("web01");
    web.mirror_key = Some("web01".to_owned());

    let mirror = read_mirror(&web, root.path()).unwrap();

    assert_eq!(mirror.synced_paths.len(), MAX_HOST_SYNCED_PATHS);
    assert!(mirror.synced_paths_truncated);
    assert_eq!(mirror.synced_paths[0], "/srv/app/0000");
}

#[test]
fn a_host_without_a_mirror_or_with_a_missing_folder_has_none() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(read_mirror(&host("web01"), root.path()), None);
    let mut web = host("web01");
    web.mirror_key = Some("web01".to_owned());
    assert_eq!(read_mirror(&web, root.path()), None);
}

#[test]
fn a_mirror_key_that_could_leave_the_mirror_root_is_ignored() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("inner")).unwrap();
    for key in ["../inner", "inner/..", "/etc", ".hidden", "a/b", ""] {
        let mut web = host("web01");
        web.mirror_key = Some(key.to_owned());
        assert_eq!(
            read_mirror(&web, &root.path().join("inner")),
            None,
            "{key:?}"
        );
    }
}

#[test]
fn a_manifest_that_cannot_be_read_still_reports_the_folder() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("web01")).unwrap();
    write_manifest(root.path(), "web01", "not json");
    let mut web = host("web01");
    web.mirror_key = Some("web01".to_owned());

    let mirror = read_mirror(&web, root.path()).unwrap();

    assert!(mirror.synced_paths.is_empty());
}

#[test]
fn sessions_are_matched_to_hosts_by_the_alias_the_user_typed() {
    let hosts = [host("web01"), host("web02")];
    let details = vec![HostDetails::default(), HostDetails::default()];
    let sessions = [
        listed("1", Some("web01"), true),
        listed("2", Some("root@web01"), false),
        listed("3", Some("web02.example.com"), false),
        listed("4", None, false),
    ];

    let result = build_result(&hosts, details, &sessions, 2);

    fn ids(host: &RemoteHostSummary) -> Vec<&str> {
        host.sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect()
    }
    assert_eq!(ids(&result.hosts[0]), ["1", "2"]);
    assert_eq!(ids(&result.hosts[1]), Vec::<&str>::new());
    assert!(result.hosts[0].sessions[0].is_active);
    assert!(result.hosts[0].sessions[1].attached.is_some());
    assert!(result.hosts[0].sessions[0].attached.is_none());
}

#[test]
fn the_summary_carries_the_metadata_of_the_host_and_the_total() {
    let mut web = Host::new("web01", HostSource::Warp);
    web.tags = vec!["prod".to_owned(), "web".to_owned()];
    web.root_login = RootLogin::SudoNopasswd;
    web.transport = Transport::Direct;
    let details = vec![HostDetails {
        connection: Some("ops@web01:22".to_owned()),
        mirror: None,
    }];

    let result = build_result(&[web], details, &[], 7);

    assert_eq!(result.total, 7);
    let summary = &result.hosts[0];
    assert_eq!(summary.alias, "web01");
    assert_eq!(summary.tags, ["prod", "web"]);
    assert_eq!(summary.source, RemoteHostSource::Warp);
    assert_eq!(summary.root_login, RemoteRootLogin::SudoNopasswd);
    assert_eq!(summary.transport, RemoteTransport::Direct);
    assert_eq!(summary.connection.as_deref(), Some("ops@web01:22"));
    assert!(!summary.missing);
}

#[test]
fn the_wire_form_has_no_field_that_could_hold_a_secret() {
    let result = build_result(&[host("web01")], vec![HostDetails::default()], &[], 1);

    let json = serde_json::to_value(&result).unwrap();

    let host = json["hosts"][0].as_object().unwrap();
    let mut keys: Vec<_> = host.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "alias",
            "root_login",
            "sessions",
            "source",
            "tags",
            "transport"
        ]
    );
}
