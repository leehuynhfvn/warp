use super::*;
use crate::host_directory::model::HostSource;

fn host(alias: &str, tags: &[&str]) -> Host {
    let mut host = Host::new(alias, HostSource::SshConfig);
    host.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
    host
}

fn aliases<'a>(matches: &[ServerMatch<'a>]) -> Vec<&'a str> {
    matches.iter().map(|m| m.host.alias.as_str()).collect()
}

fn fleet() -> Vec<Host> {
    vec![
        host("web02", &["prod", "web"]),
        host("web01", &["prod", "web"]),
        host("db01", &["prod", "db"]),
        host("lab-web", &["lab"]),
    ]
}

#[test]
fn an_empty_query_lists_every_host_by_alias() {
    let hosts = fleet();
    assert_eq!(
        aliases(&search(&hosts, "")),
        ["db01", "lab-web", "web01", "web02"]
    );
    assert_eq!(
        aliases(&search(&hosts, "   ")),
        ["db01", "lab-web", "web01", "web02"]
    );
}

#[test]
fn words_match_an_alias_loosely_and_report_where() {
    let hosts = fleet();
    let found = search(&hosts, "w01");
    assert_eq!(aliases(&found), ["web01"]);
    assert_eq!(found[0].fuzzy.matched_indices, [0, 3, 4]);
}

#[test]
fn the_case_of_the_query_does_not_matter() {
    let hosts = fleet();
    assert_eq!(aliases(&search(&hosts, "DB")), ["db01"]);
}

#[test]
fn a_tag_filter_keeps_only_hosts_with_that_tag() {
    let hosts = fleet();
    assert_eq!(aliases(&search(&hosts, "tag:db")), ["db01"]);
    assert_eq!(
        aliases(&search(&hosts, "TAG:PROD")),
        ["db01", "web01", "web02"]
    );
}

#[test]
fn several_tag_filters_all_have_to_hold() {
    let hosts = fleet();
    assert_eq!(
        aliases(&search(&hosts, "tag:prod tag:web")),
        ["web01", "web02"]
    );
    assert!(search(&hosts, "tag:prod tag:lab").is_empty());
}

#[test]
fn a_tag_filter_and_a_word_combine() {
    let hosts = fleet();
    assert_eq!(aliases(&search(&hosts, "tag:prod 02")), ["web02"]);
}

#[test]
fn an_empty_tag_filter_is_ignored() {
    let hosts = fleet();
    assert_eq!(search(&hosts, "tag:").len(), 4);
}

#[test]
fn a_word_may_also_match_a_tag() {
    let hosts = fleet();
    let found = search(&hosts, "lab");
    assert_eq!(aliases(&found), ["lab-web"]);
    let by_tag = search(&hosts, "prod");
    assert_eq!(aliases(&by_tag), ["db01", "web01", "web02"]);
    assert!(by_tag.iter().all(|m| m.fuzzy.matched_indices.is_empty()));
}

#[test]
fn every_word_has_to_match() {
    let hosts = fleet();
    assert_eq!(aliases(&search(&hosts, "web 01")), ["web01"]);
    assert!(search(&hosts, "web zzz").is_empty());
}

#[test]
fn an_alias_match_ranks_above_a_tag_match() {
    let hosts = vec![host("alpha", &["web"]), host("web01", &[])];
    assert_eq!(aliases(&search(&hosts, "web")), ["web01", "alpha"]);
}

#[test]
fn a_host_that_left_the_ssh_config_is_not_offered() {
    let mut gone = host("old", &[]);
    gone.missing = true;
    let hosts = vec![gone, host("new", &[])];
    assert_eq!(aliases(&search(&hosts, "")), ["new"]);
}

#[test]
fn a_host_whose_alias_is_not_safe_to_type_is_not_offered() {
    let hosts = vec![
        host("a;b", &[]),
        host("-oProxyCommand=x", &[]),
        host("ok", &[]),
    ];
    assert_eq!(aliases(&search(&hosts, "")), ["ok"]);
    assert!(!is_connectable(&hosts[0]));
}

#[test]
fn the_settings_list_shows_missing_hosts_after_the_connectable_ones() {
    let mut gone = Host::new("web-old", HostSource::SshConfig);
    gone.missing = true;
    let hosts = [gone, Host::new("web01", HostSource::SshConfig)];

    let aliases: Vec<_> = search_all(&hosts, "web")
        .iter()
        .map(|h| h.alias.as_str())
        .collect();

    assert_eq!(aliases, ["web01", "web-old"]);
}

#[test]
fn the_settings_list_shows_every_host_for_an_empty_query() {
    let mut gone = Host::new("web-old", HostSource::SshConfig);
    gone.missing = true;
    let hosts = [gone, Host::new("web01", HostSource::SshConfig)];

    assert_eq!(search_all(&hosts, "  ").len(), 2);
}

#[test]
fn a_tag_query_never_lists_missing_hosts() {
    let mut gone = Host::new("web-old", HostSource::SshConfig);
    gone.missing = true;
    gone.tags = vec!["prod".to_owned()];

    assert!(search_all(&[gone], "tag:prod").is_empty());
}
