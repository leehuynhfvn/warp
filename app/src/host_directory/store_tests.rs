use std::fs;

use super::*;
use crate::host_directory::model::{AuthMethod, HostSource, RootLogin, Transport};

fn sample() -> Vec<Host> {
    let mut web = Host::new("web01", HostSource::SshConfig);
    web.tags = vec!["prod".to_owned(), "web".to_owned()];
    web.auth = AuthMethod::Key;
    web.root_login = RootLogin::SudoNopasswd;
    web.requiretty = true;
    web.mirror_key = Some("draff3".to_owned());
    web.machine_id = Some("0123456789abcdef0123456789abcdef".to_owned());
    let mut lab = Host::new("lab", HostSource::Warp);
    lab.transport = Transport::Direct;
    lab.missing = true;
    vec![web, lab]
}

#[test]
fn round_trips_through_toml() {
    let hosts = sample();
    let text = serialize(&hosts).unwrap();
    assert_eq!(parse(&text).unwrap(), hosts);
}

#[test]
fn an_empty_file_with_only_a_version_is_an_empty_directory() {
    assert_eq!(parse("version = 1\n").unwrap(), Vec::new());
}

#[test]
fn rejects_a_version_it_does_not_know() {
    assert!(parse("version = 2\n").unwrap_err().contains("version 2"));
}

#[test]
fn rejects_a_file_without_a_version() {
    assert!(parse("[[hosts]]\nalias = \"a\"\nsource = \"warp\"\n").is_err());
}

#[test]
fn rejects_unknown_fields() {
    let text = "version = 1\n[[hosts]]\nalias = \"a\"\nsource = \"warp\"\npassword = \"x\"\n";
    assert!(parse(text).is_err());
}

#[test]
fn rejects_an_unknown_source() {
    let text = "version = 1\n[[hosts]]\nalias = \"a\"\nsource = \"cloud\"\n";
    assert!(parse(text).is_err());
}

#[test]
fn rejects_a_duplicate_alias() {
    let text = "version = 1\n[[hosts]]\nalias = \"a\"\nsource = \"warp\"\n[[hosts]]\nalias = \"a\"\nsource = \"ssh_config\"\n";
    assert!(parse(text).unwrap_err().contains("twice"));
}

#[test]
fn defaults_fill_in_the_optional_fields() {
    let hosts = parse("version = 1\n[[hosts]]\nalias = \"a\"\nsource = \"ssh_config\"\n").unwrap();
    assert_eq!(hosts, vec![Host::new("a", HostSource::SshConfig)]);
}

#[test]
fn an_invalid_alias_survives_a_round_trip() {
    let text = "version = 1\n[[hosts]]\nalias = \"a;b\"\nsource = \"ssh_config\"\n";
    let hosts = parse(text).unwrap();
    assert_eq!(hosts[0].alias, "a;b");
    assert_eq!(parse(&serialize(&hosts).unwrap()).unwrap(), hosts);
}

#[test]
fn a_missing_file_is_an_empty_directory() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(load(home.path()).unwrap(), Vec::new());
}

#[test]
fn saves_and_loads_from_the_home_directory() {
    let home = tempfile::tempdir().unwrap();
    save(home.path(), &sample()).unwrap();
    assert_eq!(load(home.path()).unwrap(), sample());
}

#[test]
fn the_file_and_its_directory_are_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = tempfile::tempdir().unwrap();
    save(home.path(), &sample()).unwrap();
    let path = home.path().join(HOSTS_FILE);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn saving_leaves_no_temporary_file_behind() {
    let home = tempfile::tempdir().unwrap();
    save(home.path(), &sample()).unwrap();
    save(home.path(), &[]).unwrap();
    let names: Vec<_> = fs::read_dir(home.path().join(HOSTS_FILE).parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["hosts.toml"]);
}

#[test]
fn a_file_that_does_not_parse_is_an_error_and_is_left_alone() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(HOSTS_FILE);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "this is [not toml").unwrap();
    assert!(matches!(load(home.path()), Err(HostsError::Invalid { .. })));
    assert_eq!(fs::read_to_string(&path).unwrap(), "this is [not toml");
}
