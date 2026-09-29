use super::*;

type Case = (&'static str, fn(&mut AddFormText));

fn form() -> AddFormText {
    AddFormText {
        alias: "lab-1".to_owned(),
        hostname: "203.0.113.10".to_owned(),
        ..AddFormText::default()
    }
}

#[test]
fn a_minimal_form_leaves_the_optional_fields_out() {
    let new = build_new_host(&form()).unwrap();

    assert_eq!(new.alias, "lab-1");
    assert_eq!(new.hostname, "203.0.113.10");
    assert_eq!(new.user, None);
    assert_eq!(new.port, None);
    assert_eq!(new.identity_file, None);
    assert_eq!(new.proxy_jump, None);
    assert!(new.tags.is_empty());
}

#[test]
fn every_field_is_trimmed_and_tags_are_split() {
    let new = build_new_host(&AddFormText {
        alias: " lab-1 ".to_owned(),
        hostname: " lab.example.com ".to_owned(),
        user: " ops ".to_owned(),
        port: " 2222 ".to_owned(),
        identity_file: " ~/.ssh/id_ed25519 ".to_owned(),
        proxy_jump: " gw@jump.example.com ".to_owned(),
        tags: "Prod, web  eu".to_owned(),
    })
    .unwrap();

    assert_eq!(new.alias, "lab-1");
    assert_eq!(new.hostname, "lab.example.com");
    assert_eq!(new.user.as_deref(), Some("ops"));
    assert_eq!(new.port, Some(2222));
    assert_eq!(new.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
    assert_eq!(new.proxy_jump.as_deref(), Some("gw@jump.example.com"));
    assert_eq!(new.tags, ["prod", "web", "eu"]);
}

#[test]
fn refuses_what_could_not_be_written_as_one_block() {
    let bad: [Case; 7] = [
        ("empty alias", |f| f.alias.clear()),
        ("option as alias", |f| {
            f.alias = "-oProxyCommand=x".to_owned()
        }),
        ("empty host name", |f| f.hostname.clear()),
        ("second line in host name", |f| {
            f.hostname = "x\n ProxyCommand y".to_owned()
        }),
        ("port zero", |f| f.port = "0".to_owned()),
        ("port not a number", |f| f.port = "ssh".to_owned()),
        ("port too large", |f| f.port = "70000".to_owned()),
    ];
    for (what, break_it) in bad {
        let mut candidate = form();
        break_it(&mut candidate);
        assert!(
            build_new_host(&candidate).is_err(),
            "{what} should be refused"
        );
    }
}

#[test]
fn a_tag_that_is_not_a_word_is_refused() {
    let mut candidate = form();
    candidate.tags = "ok, bad!".to_owned();

    assert!(build_new_host(&candidate).is_err());
}
