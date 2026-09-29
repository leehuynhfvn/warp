use super::*;

fn entry(glob: &str, language: &str) -> LanguageOverride {
    LanguageOverride {
        glob: glob.to_owned(),
        language: language.to_owned(),
    }
}

#[test]
fn an_override_wins_over_what_is_recognized() {
    let overrides = [entry("/etc/nginx/*.conf", "ini")];

    assert_eq!(
        language_for_path(&overrides, "/etc/nginx/nginx.conf", None),
        Some("ini")
    );
}

#[test]
fn a_file_without_a_matching_override_is_recognized_as_usual() {
    let overrides = [entry("/etc/haproxy/*.cfg", "ini")];

    assert_eq!(
        language_for_path(&overrides, "/etc/nginx/nginx.conf", None),
        Some("nginx")
    );
    assert_eq!(
        language_for_path(&overrides, "/etc/haproxy/haproxy.cfg", None),
        Some("ini")
    );
    assert_eq!(language_for_path(&overrides, "/etc/hosts", None), None);
}

#[test]
fn an_override_may_use_an_alias_and_an_unsupported_one_is_skipped() {
    let overrides = [
        entry("/opt/app/run", "khong-co"),
        entry("/opt/app/*", "bash"),
    ];

    assert_eq!(
        language_for_path(&overrides, "/opt/app/run", None),
        Some("shell")
    );
}

#[test]
fn the_first_matching_override_wins() {
    let overrides = [entry("/etc/app.conf", "toml"), entry("/etc/*", "ini")];

    assert_eq!(
        language_for_path(&overrides, "/etc/app.conf", None),
        Some("toml")
    );
}

#[test]
fn choosing_a_language_puts_it_first_and_replaces_the_previous_choice() {
    let overrides = [entry("/etc/*", "ini"), entry("/etc/app.conf", "toml")];

    let updated = with_choice(&overrides, "/etc/app.conf", Some("yaml"));

    assert_eq!(
        updated,
        [entry("/etc/app.conf", "yaml"), entry("/etc/*", "ini")]
    );
}

#[test]
fn choosing_auto_detect_removes_the_choice() {
    let overrides = [entry("/etc/app.conf", "toml"), entry("/etc/*", "ini")];

    assert_eq!(
        with_choice(&overrides, "/etc/app.conf", None),
        [entry("/etc/*", "ini")]
    );
}
