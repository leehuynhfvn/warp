use std::path::Path;

use super::*;

fn host(alias: &str) -> NewHost {
    NewHost {
        alias: alias.to_owned(),
        hostname: "203.0.113.9".to_owned(),
        ..NewHost::default()
    }
}

fn full_host() -> NewHost {
    NewHost {
        alias: "web-new".to_owned(),
        hostname: "web.example.com".to_owned(),
        user: Some("ops".to_owned()),
        port: Some(2222),
        identity_file: Some("~/.ssh/id_ed25519".to_owned()),
        proxy_jump: Some("gw@jump.example:22".to_owned()),
        tags: vec!["prod".to_owned(), "web".to_owned()],
    }
}

type Case = (&'static str, fn(&mut NewHost));

fn tags(list: &[&str]) -> Vec<String> {
    list.iter().map(|tag| (*tag).to_owned()).collect()
}

#[test]
fn renders_a_minimal_block() {
    assert_eq!(
        render_block(&host("lab")),
        "Host lab\n    HostName 203.0.113.9\n"
    );
}

#[test]
fn renders_every_field_and_the_tags_comment() {
    assert_eq!(
        render_block(&full_host()),
        "# warp:tags=prod,web\n\
         Host web-new\n\
         \x20   HostName web.example.com\n\
         \x20   User ops\n\
         \x20   Port 2222\n\
         \x20   IdentityFile ~/.ssh/id_ed25519\n\
         \x20   ProxyJump gw@jump.example:22\n"
    );
}

#[test]
fn accepts_an_ordinary_host() {
    assert_eq!(validate(&full_host()), Ok(()));
    assert_eq!(validate(&host("lab")), Ok(()));
}

#[test]
fn rejects_anything_that_could_add_a_line_or_an_option() {
    let cases: [Case; 14] = [
        ("alias with a space", |h| h.alias = "a b".to_owned()),
        ("alias that is an option", |h| h.alias = "-oX".to_owned()),
        ("empty host name", |h| h.hostname = String::new()),
        ("host name with a newline", |h| {
            h.hostname = "x\n ProxyCommand id".to_owned()
        }),
        ("host name with a space", |h| h.hostname = "x y".to_owned()),
        ("host name with a semicolon", |h| {
            h.hostname = "x;id".to_owned()
        }),
        ("host name that is an option", |h| {
            h.hostname = "-oProxyCommand=x".to_owned()
        }),
        ("user with an at sign", |h| h.user = Some("a@b".to_owned())),
        ("user with a newline", |h| h.user = Some("a\nb".to_owned())),
        ("port zero", |h| h.port = Some(0)),
        ("key file with a space", |h| {
            h.identity_file = Some("a b".to_owned())
        }),
        ("key file with a quote", |h| {
            h.identity_file = Some("a\"b".to_owned())
        }),
        ("jump host with a newline", |h| {
            h.proxy_jump = Some("a\nb".to_owned())
        }),
        ("jump host that is an option", |h| {
            h.proxy_jump = Some("-oX".to_owned())
        }),
    ];
    for (what, break_it) in cases {
        let mut candidate = full_host();
        break_it(&mut candidate);
        assert!(validate(&candidate).is_err(), "{what} should be refused");
    }
}

#[test]
fn rejects_a_bad_tag() {
    let mut candidate = full_host();
    candidate.tags = tags(&["ok", "bad tag"]);
    assert!(validate(&candidate).is_err());
}

#[test]
fn a_rendered_block_is_read_back_the_same() {
    let text = append_block("", &render_block(&full_host()));
    let blocks = parse_blocks(&text);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].alias, "web-new");
    assert_eq!(blocks[0].tags, ["prod", "web"]);
}

#[test]
fn a_new_file_starts_with_a_header_and_an_existing_one_is_kept() {
    let first = append_block("", &render_block(&host("a")));
    assert!(first.starts_with("# Managed by Warp"));
    let second = append_block(&first, &render_block(&host("b")));
    assert!(second.starts_with(&first));
    assert_eq!(parse_blocks(&second).len(), 2);
    assert!(second.contains("\n\nHost b\n"));
}

#[test]
fn appending_to_a_file_without_a_final_newline_adds_one() {
    let text = append_block("Host hand\n    HostName h", &render_block(&host("b")));
    assert!(text.starts_with("Host hand\n    HostName h\n\nHost b\n"));
}

#[test]
fn parses_hosts_with_and_without_tags() {
    let text = "# warp:tags=a,b\nHost one\n    HostName 1\n\nHost two\n    HostName 2\n";
    let blocks = parse_blocks(text);
    assert_eq!(blocks[0].alias, "one");
    assert_eq!(blocks[0].tags, ["a", "b"]);
    assert_eq!(blocks[1].alias, "two");
    assert!(blocks[1].tags.is_empty());
}

#[test]
fn a_tags_comment_belongs_to_the_host_after_it() {
    let text = "Host one\n    HostName 1\n# warp:tags=x\nHost two\n    HostName 2\n";
    let blocks = parse_blocks(text);
    assert!(blocks[0].tags.is_empty());
    assert_eq!(blocks[1].tags, ["x"]);
    let without_one = remove_block(text, "one").unwrap();
    assert_eq!(without_one, "# warp:tags=x\nHost two\n    HostName 2\n");
}

#[test]
fn a_host_line_with_several_aliases_or_a_match_is_not_managed() {
    let text = "Host a b\n    User x\nMatch host c\n    User y\nHost d\n    User z\n";
    let blocks = parse_blocks(text);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].alias, "d");
}

#[test]
fn a_pattern_or_unsafe_alias_is_not_managed() {
    assert!(parse_blocks("Host *.lab\nHost a;b\nHost -x\n").is_empty());
}

#[test]
fn removing_a_block_leaves_the_others_and_one_blank_line_between() {
    let text = append_block(
        &append_block(
            &append_block("", &render_block(&host("a"))),
            &render_block(&host("b")),
        ),
        &render_block(&host("c")),
    );
    let without_b = remove_block(&text, "b").unwrap();
    assert!(!without_b.contains("Host b"));
    assert!(without_b.contains("Host a\n    HostName 203.0.113.9\n\nHost c\n"));
    assert!(!without_b.contains("\n\n\n"));
}

#[test]
fn removing_the_last_block_leaves_the_rest_intact() {
    let text = append_block(
        &append_block("", &render_block(&host("a"))),
        &render_block(&host("b")),
    );
    let without_b = remove_block(&text, "b").unwrap();
    assert_eq!(parse_blocks(&without_b).len(), 1);
    assert!(without_b.starts_with("# Managed by Warp"));
}

#[test]
fn removing_a_block_keeps_options_the_user_added_to_other_blocks() {
    let text = "Host a\n    HostName 1\n    ServerAliveInterval 30\n\nHost b\n    HostName 2\n";
    assert_eq!(
        remove_block(text, "b").unwrap(),
        "Host a\n    HostName 1\n    ServerAliveInterval 30\n"
    );
}

#[test]
fn removing_an_unknown_alias_is_none() {
    assert_eq!(remove_block("Host a\n", "b"), None);
}

#[test]
fn setting_tags_adds_replaces_and_removes_the_comment() {
    let plain = "Host a\n    HostName 1\n";
    let added = set_tags(plain, "a", &tags(&["prod", "web"])).unwrap();
    assert_eq!(added, "# warp:tags=prod,web\nHost a\n    HostName 1\n");
    let replaced = set_tags(&added, "a", &tags(&["lab"])).unwrap();
    assert_eq!(replaced, "# warp:tags=lab\nHost a\n    HostName 1\n");
    let cleared = set_tags(&replaced, "a", &[]).unwrap();
    assert_eq!(cleared, plain);
}

#[test]
fn setting_tags_touches_only_that_block() {
    let text = "# warp:tags=x\nHost a\n    HostName 1\n\n# warp:tags=y\nHost b\n    HostName 2\n";
    let changed = set_tags(text, "a", &tags(&["z"])).unwrap();
    assert_eq!(
        changed,
        "# warp:tags=z\nHost a\n    HostName 1\n\n# warp:tags=y\nHost b\n    HostName 2\n"
    );
    assert_eq!(set_tags(text, "missing", &tags(&["z"])), None);
}

const HOME: &str = "/home/u";

fn warp_conf() -> &'static Path {
    Path::new("/home/u/.ssh/config.d/warp.conf")
}

#[test]
fn finds_the_include_in_every_spelling_that_names_the_file() {
    for line in [
        "Include config.d/warp.conf",
        "Include ~/.ssh/config.d/warp.conf",
        "Include /home/u/.ssh/config.d/warp.conf",
        "Include other config.d/warp.conf",
        "include=config.d/warp.conf",
        "Include ./config.d/warp.conf",
    ] {
        let text = format!("# mine\n{line}\nHost a\n");
        assert!(
            has_warp_include(&text, warp_conf(), Path::new(HOME)),
            "{line}"
        );
    }
}

#[test]
fn an_include_after_a_host_or_match_does_not_count() {
    for text in [
        "Host a\nInclude config.d/warp.conf\n",
        "Match all\nInclude config.d/warp.conf\n",
    ] {
        assert!(!has_warp_include(text, warp_conf(), Path::new(HOME)));
    }
}

#[test]
fn a_glob_or_another_file_is_not_the_include() {
    for text in [
        "Include config.d/*\n",
        "Include config.d/other.conf\n",
        "Include ~/.ssh/config.d/warp.conf.bak\n",
        "",
    ] {
        assert!(!has_warp_include(text, warp_conf(), Path::new(HOME)));
    }
}

#[test]
fn adding_the_include_changes_nothing_but_the_top() {
    for original in [
        "Host a\n    User x\n",
        "Host a\n    User x",
        "# comment\r\nHost a\r\n",
        "\n\nHost a\n",
        "",
    ] {
        let updated = with_warp_include(original);
        assert!(updated.starts_with(INCLUDE_LINE));
        assert!(updated.ends_with(original));
        assert!(has_warp_include(&updated, warp_conf(), Path::new(HOME)));
    }
}
