use std::path::{Path, PathBuf};

use super::*;

const WARP_CONF: &str = "/home/u/.ssh/config.d/warp.conf";

fn found(alias: &str) -> DiscoveredHost {
    found_in(alias, "/home/u/.ssh/config")
}

fn found_in(alias: &str, file: &str) -> DiscoveredHost {
    DiscoveredHost {
        alias: alias.to_owned(),
        file: PathBuf::from(file),
        line: 1,
    }
}

fn merge(existing: &[Host], discovered: &[DiscoveredHost]) -> (Vec<Host>, MergeReport) {
    merge_discovered(existing, discovered, Path::new(WARP_CONF))
}

#[test]
fn accepts_ordinary_aliases() {
    for alias in [
        "web01",
        "db-sql01",
        "mail.runsystem.vn",
        "a",
        "Web_1",
        "9lives",
    ] {
        assert_eq!(validate_alias(alias), Ok(()), "{alias}");
    }
}

#[test]
fn rejects_aliases_that_a_shell_or_ssh_would_act_on() {
    let cases = [
        ("", NameError::Empty),
        ("-oProxyCommand=x", NameError::BadStart),
        ("-v", NameError::BadStart),
        (".hidden", NameError::BadStart),
        ("a b", NameError::BadCharacter),
        ("a;rm", NameError::BadCharacter),
        ("a$(id)", NameError::BadCharacter),
        ("a`id`", NameError::BadCharacter),
        ("a|b", NameError::BadCharacter),
        ("a&b", NameError::BadCharacter),
        ("a>b", NameError::BadCharacter),
        ("a\nb", NameError::BadCharacter),
        ("a'b", NameError::BadCharacter),
        ("a\"b", NameError::BadCharacter),
        ("a*", NameError::BadCharacter),
        ("a/b", NameError::BadCharacter),
        ("máy", NameError::BadCharacter),
        ("web-é", NameError::BadCharacter),
    ];
    for (alias, expected) in cases {
        assert_eq!(validate_alias(alias), Err(expected), "{alias:?}");
    }
}

#[test]
fn limits_the_length_of_an_alias() {
    assert_eq!(validate_alias(&"a".repeat(64)), Ok(()));
    assert_eq!(validate_alias(&"a".repeat(65)), Err(NameError::TooLong));
}

#[test]
fn a_new_alias_is_added_as_an_ssh_config_host() {
    let (hosts, report) = merge(&[], &[found("web01"), found("web02")]);
    assert_eq!(
        hosts,
        vec![
            Host::new("web01", HostSource::SshConfig),
            Host::new("web02", HostSource::SshConfig)
        ]
    );
    assert_eq!(report.added, ["web01", "web02"]);
    assert!(report.missing.is_empty() && report.restored.is_empty());
}

#[test]
fn an_alias_from_the_warp_file_is_adopted_as_a_warp_host() {
    let (hosts, _) = merge(&[], &[found_in("lab", WARP_CONF)]);
    assert_eq!(hosts[0].source, HostSource::Warp);
}

#[test]
fn a_known_alias_keeps_its_metadata() {
    let mut web = Host::new("web01", HostSource::SshConfig);
    web.tags = vec!["prod".to_owned()];
    web.root_login = RootLogin::SudoNopasswd;
    let (hosts, report) = merge(std::slice::from_ref(&web), &[found("web01")]);
    assert_eq!(hosts, vec![web]);
    assert_eq!(report, MergeReport::default());
}

#[test]
fn an_alias_that_disappeared_is_marked_missing_not_removed() {
    let mut web = Host::new("web01", HostSource::SshConfig);
    web.tags = vec!["prod".to_owned()];
    let (hosts, report) = merge(&[web], &[found("web02")]);
    assert_eq!(hosts.len(), 2);
    assert!(hosts[0].missing);
    assert_eq!(hosts[0].tags, ["prod"]);
    assert_eq!(report.missing, ["web01"]);
    assert_eq!(report.added, ["web02"]);
}

#[test]
fn a_missing_alias_that_comes_back_is_restored_with_its_metadata() {
    let mut web = Host::new("web01", HostSource::SshConfig);
    web.tags = vec!["prod".to_owned()];
    web.missing = true;
    let (hosts, report) = merge(&[web], &[found("web01")]);
    assert!(!hosts[0].missing);
    assert_eq!(hosts[0].tags, ["prod"]);
    assert_eq!(report.restored, ["web01"]);
}

#[test]
fn a_missing_host_is_reported_only_once() {
    let web = Host::new("web01", HostSource::SshConfig);
    let (first, _) = merge(&[web], &[]);
    let (second, report) = merge(&first, &[]);
    assert_eq!(first, second);
    assert_eq!(report, MergeReport::default());
}

#[test]
fn a_host_created_in_warp_is_never_marked_missing() {
    let lab = Host::new("lab", HostSource::Warp);
    let (hosts, report) = merge(&[lab], &[]);
    assert!(!hosts[0].missing);
    assert_eq!(report, MergeReport::default());
}

#[test]
fn a_known_host_does_not_change_source_when_found_elsewhere() {
    let web = Host::new("web01", HostSource::SshConfig);
    let (hosts, _) = merge(&[web], &[found_in("web01", WARP_CONF)]);
    assert_eq!(hosts[0].source, HostSource::SshConfig);
}

#[test]
fn the_order_of_existing_hosts_is_kept_and_new_ones_are_appended() {
    let existing = [
        Host::new("b", HostSource::SshConfig),
        Host::new("a", HostSource::SshConfig),
    ];
    let (hosts, _) = merge(&existing, &[found("a"), found("c"), found("b")]);
    let aliases: Vec<&str> = hosts.iter().map(|host| host.alias.as_str()).collect();
    assert_eq!(aliases, ["b", "a", "c"]);
}

#[test]
fn an_alias_found_twice_is_added_once() {
    let (hosts, report) = merge(&[], &[found("web01"), found("web01")]);
    assert_eq!(hosts.len(), 1);
    assert_eq!(report.added, ["web01"]);
}

#[test]
fn a_tag_is_a_single_plain_word() {
    assert_eq!(validate_tag("prod"), Ok(()));
    assert_eq!(validate_tag("project-x"), Ok(()));
    assert_eq!(validate_tag("a,b"), Err(NameError::BadCharacter));
    assert_eq!(validate_tag("a b"), Err(NameError::BadCharacter));
    assert_eq!(validate_tag("a=b"), Err(NameError::BadCharacter));
    assert_eq!(validate_tag(&"t".repeat(33)), Err(NameError::TooLong));
}

#[test]
fn tags_are_split_on_commas_and_white_space_lowercased_and_deduplicated() {
    assert_eq!(
        parse_tags("Prod, web  db,prod").unwrap(),
        ["prod", "web", "db"]
    );
    assert_eq!(parse_tags("").unwrap(), Vec::<String>::new());
    assert_eq!(parse_tags(" , ,").unwrap(), Vec::<String>::new());
}

#[test]
fn a_bad_tag_names_itself_in_the_error() {
    let error = parse_tags("ok bad=tag").unwrap_err();
    assert!(error.contains("bad=tag"), "{error}");
}

#[test]
fn a_host_may_have_at_most_sixteen_tags() {
    let many: Vec<String> = (0..17).map(|n| format!("t{n}")).collect();
    assert!(parse_tags(&many.join(",")).is_err());
    assert_eq!(parse_tags(&many[..16].join(",")).unwrap().len(), 16);
}
