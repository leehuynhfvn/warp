use super::*;
use crate::host_directory::HostSource;

fn host(mirror_key: Option<&str>, machine_id: Option<&str>) -> Host {
    Host {
        mirror_key: mirror_key.map(str::to_owned),
        machine_id: machine_id.map(str::to_owned),
        ..Host::new("web01", HostSource::SshConfig)
    }
}

fn link(mirror_key: &str, machine_id: Option<&str>) -> MirrorLink {
    MirrorLink {
        mirror_key: mirror_key.to_owned(),
        machine_id: machine_id.map(str::to_owned),
    }
}

#[test]
fn a_host_without_a_mirror_learns_it() {
    let update = mirror_link_update(&host(None, None), &link("web01", Some("m1")));
    assert_eq!(update, MirrorUpdate::Set(link("web01", Some("m1"))));
}

#[test]
fn the_same_mirror_and_machine_changes_nothing() {
    let update = mirror_link_update(&host(Some("web01"), Some("m1")), &link("web01", Some("m1")));
    assert_eq!(update, MirrorUpdate::Keep);
}

#[test]
fn another_machine_id_warns_and_takes_the_new_mirror() {
    let update = mirror_link_update(
        &host(Some("web01"), Some("m1")),
        &link("web01-abcd", Some("m2")),
    );
    assert_eq!(update, MirrorUpdate::Warn(link("web01-abcd", Some("m2"))));
}

#[test]
fn a_machine_id_is_filled_in_when_the_host_had_none() {
    let update = mirror_link_update(&host(Some("web01"), None), &link("web01", Some("m1")));
    assert_eq!(update, MirrorUpdate::Set(link("web01", Some("m1"))));
}

#[test]
fn a_machine_that_reports_no_id_does_not_erase_the_known_one() {
    let update = mirror_link_update(&host(Some("web01"), Some("m1")), &link("web01", None));
    assert_eq!(update, MirrorUpdate::Keep);
}

#[test]
fn a_renamed_machine_with_the_same_id_moves_to_its_new_mirror_silently() {
    let update = mirror_link_update(
        &host(Some("web01"), Some("m1")),
        &link("web01b", Some("m1")),
    );
    assert_eq!(update, MirrorUpdate::Set(link("web01b", Some("m1"))));
}

#[test]
fn a_host_without_an_id_that_still_reports_none_only_learns_the_mirror() {
    let update = mirror_link_update(&host(None, None), &link("web01", None));
    assert_eq!(update, MirrorUpdate::Set(link("web01", None)));
    let update = mirror_link_update(&host(Some("web01"), None), &link("web01", None));
    assert_eq!(update, MirrorUpdate::Keep);
}

#[test]
fn alias_of_ssh_host_drops_the_user() {
    assert_eq!(alias_of_ssh_host("web01"), "web01");
    assert_eq!(alias_of_ssh_host("root@web01"), "web01");
    assert_eq!(alias_of_ssh_host("a@b@web01"), "web01");
    assert_eq!(alias_of_ssh_host("web01@"), "");
}

fn home_with_hosts(hosts: &[Host]) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    store::save(home.path(), hosts).unwrap();
    home
}

#[test]
fn apply_writes_the_link_of_a_known_alias() {
    let home = home_with_hosts(&[host(None, None), Host::new("web02", HostSource::Warp)]);
    let applied = apply_observation(home.path(), "web01", &link("web01", Some("m1")))
        .unwrap()
        .unwrap();
    assert!(!applied.machine_changed);
    let saved = store::load(home.path()).unwrap();
    assert_eq!(saved, applied.hosts);
    assert_eq!(saved[0].mirror_key.as_deref(), Some("web01"));
    assert_eq!(saved[0].machine_id.as_deref(), Some("m1"));
    assert_eq!(saved[1], Host::new("web02", HostSource::Warp));
}

#[test]
fn apply_reports_a_machine_change_and_records_the_new_link() {
    let home = home_with_hosts(&[host(Some("web01"), Some("m1"))]);
    let applied = apply_observation(home.path(), "web01", &link("web01-abcd", Some("m2")))
        .unwrap()
        .unwrap();
    assert!(applied.machine_changed);
    let saved = store::load(home.path()).unwrap();
    assert_eq!(saved[0].mirror_key.as_deref(), Some("web01-abcd"));
    assert_eq!(saved[0].machine_id.as_deref(), Some("m2"));
}

#[test]
fn apply_reports_nothing_when_the_link_is_already_recorded() {
    let home = home_with_hosts(&[host(Some("web01"), Some("m1"))]);
    let path = home.path().join(crate::host_directory::HOSTS_FILE);
    let before = std::fs::read_to_string(&path).unwrap();
    let applied = apply_observation(home.path(), "web01", &link("web01", Some("m1"))).unwrap();
    assert_eq!(applied, None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn apply_ignores_an_alias_that_is_not_in_the_directory() {
    let home = home_with_hosts(&[host(None, None)]);
    let applied = apply_observation(home.path(), "other", &link("other", Some("m1"))).unwrap();
    assert_eq!(applied, None);
    assert_eq!(store::load(home.path()).unwrap(), [host(None, None)]);
}

#[test]
fn apply_leaves_a_corrupt_file_alone() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(crate::host_directory::HOSTS_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "not [ valid").unwrap();
    assert!(apply_observation(home.path(), "web01", &link("web01", None)).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not [ valid");
}

#[test]
fn mirror_dir_joins_a_plain_key_to_the_mirror_root() {
    let root = Path::new("/mirrors");
    assert_eq!(
        mirror_dir(&host(Some("web01-ab12"), None), root),
        Some(PathBuf::from("/mirrors/web01-ab12"))
    );
}

#[test]
fn mirror_dir_refuses_a_key_that_could_leave_the_mirror_root() {
    let root = Path::new("/mirrors");
    assert_eq!(mirror_dir(&host(None, None), root), None);
    for key in ["../etc", "a/b", "/etc", ".hidden", ""] {
        assert_eq!(mirror_dir(&host(Some(key), None), root), None, "{key:?}");
    }
}
