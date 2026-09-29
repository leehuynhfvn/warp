use std::fs;
use std::path::Path;

use sha2::{Digest as _, Sha256};

use super::*;
use crate::host_directory::HOSTS_FILE;

fn home_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    for (name, contents) in files {
        let path = home.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    home
}

fn digest(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).unwrap()))
}

fn aliases(refreshed: &Refreshed) -> Vec<&str> {
    refreshed
        .hosts
        .iter()
        .map(|host| host.alias.as_str())
        .collect()
}

const CONFIG: &str = ".ssh/config";

#[test]
fn the_first_scan_imports_every_concrete_alias_and_writes_the_file() {
    let home = home_with(&[(CONFIG, "Host web01 web02 *.lab\nHost *\n  User root\n")]);
    let refreshed = refresh_blocking(home.path()).unwrap();
    assert_eq!(aliases(&refreshed), ["web01", "web02"]);
    assert!(refreshed.first_import);
    assert_eq!(refreshed.report.added, ["web01", "web02"]);
    assert_eq!(store::load(home.path()).unwrap(), refreshed.hosts);
}

#[test]
fn a_scan_never_changes_the_ssh_configuration() {
    let home = home_with(&[
        (CONFIG, "Include config.work\nHost web01\n"),
        (".ssh/config.work", "Host work01\n"),
    ]);
    let before = (
        digest(&home.path().join(CONFIG)),
        digest(&home.path().join(".ssh/config.work")),
    );
    refresh_blocking(home.path()).unwrap();
    refresh_blocking(home.path()).unwrap();
    let after = (
        digest(&home.path().join(CONFIG)),
        digest(&home.path().join(".ssh/config.work")),
    );
    assert_eq!(before, after);
}

#[test]
fn a_scan_with_nothing_new_does_not_rewrite_the_file() {
    let home = home_with(&[(CONFIG, "Host web01\n")]);
    refresh_blocking(home.path()).unwrap();
    let path = home.path().join(HOSTS_FILE);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let second = refresh_blocking(home.path()).unwrap();
    assert_eq!(second.report, MergeReport::default());
    assert!(!second.first_import);
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
}

#[test]
fn metadata_typed_by_the_user_survives_a_scan() {
    let home = home_with(&[(CONFIG, "Host web01\n")]);
    let mut hosts = refresh_blocking(home.path()).unwrap().hosts;
    hosts[0].tags = vec!["prod".to_owned()];
    store::save(home.path(), &hosts).unwrap();
    let refreshed = refresh_blocking(home.path()).unwrap();
    assert_eq!(refreshed.hosts[0].tags, ["prod"]);
}

#[test]
fn a_host_removed_from_the_configuration_is_kept_as_missing() {
    let home = home_with(&[(CONFIG, "Host web01\nHost web02\n")]);
    refresh_blocking(home.path()).unwrap();
    fs::write(home.path().join(CONFIG), "Host web02\n").unwrap();
    let refreshed = refresh_blocking(home.path()).unwrap();
    assert_eq!(refreshed.report.missing, ["web01"]);
    assert!(refreshed.hosts[0].missing);
    assert!(store::load(home.path()).unwrap()[0].missing);
}

#[test]
fn a_hosts_file_that_cannot_be_parsed_is_reported_and_left_alone() {
    let home = home_with(&[(CONFIG, "Host web01\n"), (HOSTS_FILE, "version = [")]);
    let error = refresh_blocking(home.path()).unwrap_err();
    assert!(matches!(error, HostsError::Invalid { .. }));
    assert_eq!(
        fs::read_to_string(home.path().join(HOSTS_FILE)).unwrap(),
        "version = ["
    );
}

#[test]
fn a_host_written_by_warp_is_adopted_from_its_own_file() {
    let home = home_with(&[
        (CONFIG, "Include config.d/warp.conf\nHost web01\n"),
        (".ssh/config.d/warp.conf", "# warp:tags=lab\nHost lab1\n"),
    ]);
    let refreshed = refresh_blocking(home.path()).unwrap();
    let lab = refreshed
        .hosts
        .iter()
        .find(|host| host.alias == "lab1")
        .unwrap();
    assert_eq!(lab.source, crate::host_directory::model::HostSource::Warp);
}

fn refreshed(
    added: usize,
    missing: usize,
    restored: usize,
    total: usize,
    first: bool,
) -> Refreshed {
    let names = |prefix: &str, count: usize| (0..count).map(|n| format!("{prefix}{n}")).collect();
    Refreshed {
        hosts: (0..total)
            .map(|n| {
                crate::host_directory::model::Host::new(
                    format!("h{n}"),
                    crate::host_directory::model::HostSource::SshConfig,
                )
            })
            .collect(),
        report: MergeReport {
            added: names("a", added),
            missing: names("m", missing),
            restored: names("r", restored),
        },
        first_import: first,
        warnings: Vec::new(),
        files: Vec::new(),
    }
}

fn message(mode: RefreshMode, refreshed: Refreshed) -> Option<String> {
    refresh_message(mode, &Ok(refreshed))
}

#[test]
fn the_first_import_is_one_message_not_one_per_host() {
    assert_eq!(
        message(RefreshMode::Startup, refreshed(179, 0, 0, 179, true)).as_deref(),
        Some("Imported 179 SSH hosts")
    );
    assert_eq!(
        message(RefreshMode::Startup, refreshed(1, 0, 0, 1, true)).as_deref(),
        Some("Imported 1 SSH host")
    );
}

#[test]
fn new_hosts_after_the_first_import_are_announced() {
    assert_eq!(
        message(RefreshMode::Startup, refreshed(1, 0, 0, 180, false)).as_deref(),
        Some("Found 1 new SSH host")
    );
    assert_eq!(
        message(RefreshMode::Startup, refreshed(3, 0, 0, 182, false)).as_deref(),
        Some("Found 3 new SSH hosts")
    );
}

#[test]
fn startup_is_silent_when_there_is_no_news() {
    assert_eq!(
        message(RefreshMode::Startup, refreshed(0, 0, 0, 5, false)),
        None
    );
    assert_eq!(
        message(RefreshMode::Startup, refreshed(0, 2, 0, 5, false)),
        None
    );
}

#[test]
fn a_manual_scan_always_answers() {
    assert_eq!(
        message(RefreshMode::Manual, refreshed(0, 0, 0, 5, false)).as_deref(),
        Some("No changes; 5 hosts in the server list")
    );
    assert_eq!(
        message(RefreshMode::Manual, refreshed(0, 0, 0, 1, false)).as_deref(),
        Some("No changes; 1 host in the server list")
    );
    assert_eq!(
        message(RefreshMode::Manual, refreshed(1, 2, 1, 9, false)).as_deref(),
        Some(
            "Found 1 new SSH host; 2 hosts no longer in the SSH config; 1 host back in the SSH config"
        )
    );
}

#[test]
fn a_failure_is_always_reported() {
    for mode in [RefreshMode::Startup, RefreshMode::Manual] {
        let message = refresh_message(mode, &Err("bad file".to_owned())).unwrap();
        assert_eq!(message, "Could not update the server list: bad file");
    }
}
