use super::*;

fn parse(text: &str) -> Policy {
    Policy::parse(text).expect("policy parses")
}

fn parse_err(text: &str) -> PolicyError {
    Policy::parse(text).expect_err("policy is rejected")
}

fn decide(policy: &Policy, host: &str, request: PolicyRequest<'_>) -> Decision {
    policy.evaluate(host, request, false)
}

fn deny_message(decision: &Decision) -> &str {
    match decision {
        Decision::Deny(reason) => reason,
        other => panic!("expected Deny, got {other:?}"),
    }
}

const MINIMAL_APPROVE: &str = r#"
[defaults]
mode = "approve"
"#;

// --- Parsing / validation -------------------------------------------------

#[test]
fn missing_defaults_table_is_an_error() {
    assert!(matches!(
        parse_err("[deny]\npatterns = []\n"),
        PolicyError::Parse(_)
    ));
}

#[test]
fn missing_mode_in_defaults_is_an_error() {
    assert!(matches!(
        parse_err("[defaults]\nrequire_pairing = false\n"),
        PolicyError::Parse(_)
    ));
}

#[test]
fn invalid_mode_value_is_an_error() {
    let text = "[defaults]\nmode = \"sometimes\"\n";
    assert!(matches!(parse_err(text), PolicyError::Parse(_)));
}

#[test]
fn unknown_top_level_key_is_an_error() {
    let text = "[defaults]\nmode = \"approve\"\n\nfoo = 1\n";
    assert!(matches!(parse_err(text), PolicyError::Parse(_)));
}

#[test]
fn unknown_key_in_defaults_is_an_error() {
    let text = "[defaults]\nmode = \"approve\"\ntypo_field = true\n";
    assert!(matches!(parse_err(text), PolicyError::Parse(_)));
}

#[test]
fn unknown_key_in_host_rule_is_an_error() {
    let text = r#"
[defaults]
mode = "approve"

[[hosts]]
match = "lab-*"
mode = "approve"
oops = 1
"#;
    assert!(matches!(parse_err(text), PolicyError::Parse(_)));
}

#[test]
fn empty_host_match_is_rejected() {
    let text = r#"
[defaults]
mode = "approve"

[[hosts]]
match = ""
mode = "approve"
"#;
    assert!(matches!(parse_err(text), PolicyError::Invalid(_)));
}

#[test]
fn empty_allow_entry_is_rejected() {
    let text = r#"
[defaults]
mode = "allowlist"
allow = [""]
"#;
    assert!(matches!(parse_err(text), PolicyError::Invalid(_)));
}

#[test]
fn allow_entry_with_a_metacharacter_is_rejected() {
    for bad in [';', '|', '&', '$', '`', '<', '>', '(', ')'] {
        let text = format!("[defaults]\nmode = \"allowlist\"\nallow = [\"id{bad}reboot\"]\n");
        match parse_err(&text) {
            PolicyError::Invalid(message) => assert!(
                message.contains(&bad.to_string()),
                "message '{message}' should name the bad character '{bad}'"
            ),
            other => panic!("expected Invalid for '{bad}', got {other:?}"),
        }
    }
}

#[test]
fn deny_pattern_that_does_not_compile_names_the_pattern_in_the_error() {
    let text = "[defaults]\nmode = \"approve\"\n\n[deny]\npatterns = [\"[unclosed\"]\n";
    match parse_err(text) {
        PolicyError::Invalid(message) => assert!(message.contains("[unclosed")),
        other => panic!("expected Invalid, got {other:?}"),
    }
}

// --- Mode behavior, one request kind at a time ----------------------------

#[test]
fn read_only_denies_exec() {
    let policy = parse("[defaults]\nmode = \"read_only\"\n");
    let decision = decide(&policy, "prod-1", PolicyRequest::Exec("id"));
    assert!(deny_message(&decision).contains("prod-1 is read-only"));
}

#[test]
fn read_only_denies_exec_visible() {
    let policy = parse("[defaults]\nmode = \"read_only\"\n");
    let decision = decide(&policy, "prod-1", PolicyRequest::ExecVisible("df -h"));
    assert!(deny_message(&decision).contains("read-only"));
}

#[test]
fn read_only_denies_write() {
    let policy = parse("[defaults]\nmode = \"read_only\"\n");
    let decision = decide(&policy, "prod-1", PolicyRequest::Write { path: "/tmp/x" });
    assert!(deny_message(&decision).contains("read-only"));
}

#[test]
fn approve_asks_for_exec() {
    let policy = parse(MINIMAL_APPROVE);
    assert_eq!(
        decide(&policy, "prod-1", PolicyRequest::Exec("id")),
        Decision::Ask
    );
}

#[test]
fn approve_asks_for_exec_visible() {
    let policy = parse(MINIMAL_APPROVE);
    assert_eq!(
        decide(&policy, "prod-1", PolicyRequest::ExecVisible("df -h")),
        Decision::Ask
    );
}

#[test]
fn approve_asks_for_write() {
    let policy = parse(MINIMAL_APPROVE);
    let decision = decide(&policy, "prod-1", PolicyRequest::Write { path: "/tmp/x" });
    assert_eq!(decision, Decision::Ask);
}

#[test]
fn allowlist_allows_an_exact_match_exec() {
    let policy = parse("[defaults]\nmode = \"allowlist\"\nallow = [\"id -un\"]\n");
    assert_eq!(
        decide(&policy, "prod-1", PolicyRequest::Exec("id -un")),
        Decision::Allow
    );
}

#[test]
fn allowlist_allows_an_exact_match_exec_visible() {
    let policy = parse("[defaults]\nmode = \"allowlist\"\nallow = [\"uptime\"]\n");
    assert_eq!(
        decide(&policy, "prod-1", PolicyRequest::ExecVisible("uptime")),
        Decision::Allow
    );
}

#[test]
fn allowlist_trims_whitespace_before_matching() {
    let policy = parse("[defaults]\nmode = \"allowlist\"\nallow = [\"id -un\"]\n");
    assert_eq!(
        decide(&policy, "prod-1", PolicyRequest::Exec(" id -un ")),
        Decision::Allow
    );
}

#[test]
fn allowlist_asks_for_a_command_not_on_the_list() {
    let policy = parse("[defaults]\nmode = \"allowlist\"\nallow = [\"id -un\"]\n");
    assert_eq!(
        decide(&policy, "prod-1", PolicyRequest::Exec("id")),
        Decision::Ask
    );
}

#[test]
fn allowlist_always_asks_for_writes() {
    let policy = parse("[defaults]\nmode = \"allowlist\"\nallow = [\"id -un\"]\n");
    let decision = decide(&policy, "prod-1", PolicyRequest::Write { path: "/tmp/x" });
    assert_eq!(decision, Decision::Ask);
}

// --- Host selection --------------------------------------------------------

#[test]
fn no_host_matches_falls_back_to_defaults() {
    let text = r#"
[defaults]
mode = "read_only"

[[hosts]]
match = "lab-*"
mode = "approve"
"#;
    let policy = parse(text);
    let decision = decide(&policy, "prod-1", PolicyRequest::Exec("id"));
    assert!(deny_message(&decision).contains("read-only"));
}

#[test]
fn first_matching_host_wins() {
    let text = r#"
[defaults]
mode = "read_only"

[[hosts]]
match = "lab-*"
mode = "allowlist"
allow = ["id -un"]

[[hosts]]
match = "lab-*"
mode = "read_only"
"#;
    let policy = parse(text);
    assert_eq!(
        decide(&policy, "lab-1", PolicyRequest::Exec("id -un")),
        Decision::Allow
    );
}

#[test]
fn glob_star_matches_a_hostname_prefix() {
    let text =
        "[defaults]\nmode = \"read_only\"\n\n[[hosts]]\nmatch = \"lab-*\"\nmode = \"approve\"\n";
    let policy = parse(text);
    assert_eq!(
        decide(&policy, "lab-42", PolicyRequest::Exec("id")),
        Decision::Ask
    );
}

#[test]
fn glob_question_mark_matches_exactly_one_character() {
    let text =
        "[defaults]\nmode = \"read_only\"\n\n[[hosts]]\nmatch = \"db-?\"\nmode = \"approve\"\n";
    let policy = parse(text);
    assert_eq!(
        decide(&policy, "db-1", PolicyRequest::Exec("id")),
        Decision::Ask
    );
    let decision = decide(&policy, "db-12", PolicyRequest::Exec("id"));
    assert!(
        deny_message(&decision).contains("read-only"),
        "db-12 has two characters after db-, should not match db-?"
    );
}

#[test]
fn host_match_is_case_insensitive() {
    let text =
        "[defaults]\nmode = \"read_only\"\n\n[[hosts]]\nmatch = \"Lab-*\"\nmode = \"approve\"\n";
    let policy = parse(text);
    assert_eq!(
        decide(&policy, "LAB-1", PolicyRequest::Exec("id")),
        Decision::Ask
    );
}

// --- Deny always wins -------------------------------------------------------

#[test]
fn deny_pattern_wins_over_allowlist() {
    let text = r#"
[defaults]
mode = "allowlist"
allow = ["id -un"]

[deny]
patterns = ['\bid\b']
"#;
    let policy = parse(text);
    let decision = decide(&policy, "prod-1", PolicyRequest::Exec("id -un"));
    assert!(deny_message(&decision).contains("matches the deny rule"));
}

#[test]
fn deny_pattern_reason_names_the_pattern() {
    let text = "[defaults]\nmode = \"approve\"\n\n[deny]\npatterns = ['\\breboot\\b']\n";
    let policy = parse(text);
    let decision = decide(&policy, "prod-1", PolicyRequest::Exec("sudo reboot"));
    assert!(deny_message(&decision).contains("reboot"));
}

#[test]
fn deny_path_denies_a_write_even_in_allowlist_mode() {
    let text = r#"
[defaults]
mode = "allowlist"

[deny]
paths = ["/etc/sudoers", "/etc/sudoers.d/*"]
"#;
    let policy = parse(text);
    let decision = decide(
        &policy,
        "prod-1",
        PolicyRequest::Write {
            path: "/etc/sudoers.d/agent",
        },
    );
    assert!(deny_message(&decision).contains("matches the deny rule"));
}

#[test]
fn deny_path_that_does_not_match_lets_the_mode_decide() {
    let text = "[defaults]\nmode = \"approve\"\n\n[deny]\npaths = [\"/etc/sudoers\"]\n";
    let policy = parse(text);
    let decision = decide(&policy, "prod-1", PolicyRequest::Write { path: "/tmp/x" });
    assert_eq!(decision, Decision::Ask);
}

// --- Pairing -----------------------------------------------------------------

#[test]
fn require_pairing_denies_an_unpaired_agent() {
    let text = "[defaults]\nmode = \"approve\"\nrequire_pairing = true\n";
    let policy = Policy::parse(text).unwrap();
    let decision = policy.evaluate("prod-1", PolicyRequest::Exec("id"), false);
    assert!(deny_message(&decision).contains("not paired"));
}

#[test]
fn require_pairing_lets_a_paired_agent_through_to_the_normal_mode() {
    let text = "[defaults]\nmode = \"approve\"\nrequire_pairing = true\n";
    let policy = Policy::parse(text).unwrap();
    let decision = policy.evaluate("prod-1", PolicyRequest::Exec("id"), true);
    assert_eq!(decision, Decision::Ask);
}

#[test]
fn require_pairing_defaults_to_false() {
    let policy = parse(MINIMAL_APPROVE);
    let decision = policy.evaluate("prod-1", PolicyRequest::Exec("id"), false);
    assert_eq!(decision, Decision::Ask);
}

// --- Command length limit on Ask --------------------------------------------

#[test]
fn a_command_over_the_byte_limit_is_denied_instead_of_asked() {
    let policy = parse(MINIMAL_APPROVE);
    let long = "echo ".to_owned() + &"a".repeat(APPROVAL_MAX_COMMAND_BYTES);
    let decision = decide(&policy, "prod-1", PolicyRequest::Exec(&long));
    assert!(deny_message(&decision).contains("too long"));
}

#[test]
fn a_command_over_the_line_limit_is_denied_instead_of_asked() {
    let policy = parse(MINIMAL_APPROVE);
    let long = "echo\n".repeat(APPROVAL_MAX_COMMAND_LINES + 1);
    let decision = decide(&policy, "prod-1", PolicyRequest::Exec(&long));
    assert!(deny_message(&decision).contains("too long"));
}

#[test]
fn a_short_command_within_limits_is_still_asked() {
    let policy = parse(MINIMAL_APPROVE);
    assert_eq!(
        decide(&policy, "prod-1", PolicyRequest::Exec("id")),
        Decision::Ask
    );
}

#[test]
fn a_long_command_that_matches_the_allowlist_is_still_allowed() {
    let long = "echo ".to_owned() + &"a".repeat(APPROVAL_MAX_COMMAND_BYTES);
    let text = format!("[defaults]\nmode = \"allowlist\"\nallow = [{long:?}]\n");
    let policy = parse(&text);
    assert_eq!(
        decide(&policy, "prod-1", PolicyRequest::Exec(&long)),
        Decision::Allow
    );
}

#[test]
fn writes_are_not_subject_to_the_command_length_limit() {
    let policy = parse(MINIMAL_APPROVE);
    let decision = decide(&policy, "prod-1", PolicyRequest::Write { path: "/tmp/x" });
    assert_eq!(decision, Decision::Ask);
}

// --- Default / load ----------------------------------------------------------

#[test]
fn the_default_policy_is_approve_mode_with_no_deny_rules_or_hosts() {
    let policy = Policy::default();
    assert_eq!(
        policy.evaluate("prod-1", PolicyRequest::Exec("id"), false),
        Decision::Ask
    );
}

#[test]
fn load_with_no_file_returns_the_default_policy() {
    let home = tempfile::tempdir().unwrap();
    let policy = load(home.path()).unwrap();
    assert_eq!(policy, Policy::default());
}

#[cfg(unix)]
fn write_policy(home: &Path, text: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = home.join(".warp").join("agent-ops");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.toml");
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
#[test]
fn load_reads_a_policy_file_with_0600_permissions() {
    let home = tempfile::tempdir().unwrap();
    write_policy(home.path(), MINIMAL_APPROVE, 0o600);
    let policy = load(home.path()).unwrap();
    assert_eq!(
        policy.evaluate("prod-1", PolicyRequest::Exec("id"), false),
        Decision::Ask
    );
}

#[cfg(unix)]
#[test]
fn load_rejects_a_policy_file_readable_by_the_group() {
    let home = tempfile::tempdir().unwrap();
    write_policy(home.path(), MINIMAL_APPROVE, 0o640);
    match load(home.path()).unwrap_err() {
        PolicyError::Permissions { mode } => assert_eq!(mode, 0o640),
        other => panic!("expected Permissions, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn load_rejects_a_policy_file_readable_by_others() {
    let home = tempfile::tempdir().unwrap();
    write_policy(home.path(), MINIMAL_APPROVE, 0o604);
    assert!(matches!(
        load(home.path()).unwrap_err(),
        PolicyError::Permissions { .. }
    ));
}

#[cfg(unix)]
#[test]
fn load_propagates_a_parse_error_from_disk() {
    let home = tempfile::tempdir().unwrap();
    write_policy(home.path(), "[defaults]\nmode = \"sometimes\"\n", 0o600);
    assert!(matches!(
        load(home.path()).unwrap_err(),
        PolicyError::Parse(_)
    ));
}

// --- Opening sessions ([open]) --------------------------------------------

fn tags(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn open_request<'a>(alias: &'a str, tags: &'a [String], access: Access) -> OpenRequest<'a> {
    OpenRequest {
        alias,
        tags,
        access,
        root: false,
    }
}

fn policy_with_open(open: &str) -> Policy {
    parse(&format!("{MINIMAL_APPROVE}\n{open}"))
}

#[test]
fn a_server_with_no_open_rule_is_asked_about() {
    for text in [
        MINIMAL_APPROVE.to_owned(),
        format!("{MINIMAL_APPROVE}\n[open]\n"),
    ] {
        let policy = parse(&text);
        assert_eq!(
            policy.evaluate_open(open_request("lab-1", &[], Access::ReadOnly)),
            Decision::Ask
        );
    }
}

#[test]
fn a_missing_policy_file_asks_about_opening_too() {
    assert_eq!(
        Policy::default().evaluate_open(open_request("lab-1", &[], Access::Full)),
        Decision::Ask
    );
}

#[test]
fn defaults_mode_does_not_decide_opening() {
    let policy = parse("[defaults]\nmode = \"allowlist\"\nallow = [\"id\"]\n");
    assert_eq!(
        policy.evaluate_open(open_request("lab-1", &[], Access::ReadOnly)),
        Decision::Ask
    );
}

#[test]
fn an_allow_rule_opens_without_asking_up_to_its_max_access() {
    let policy = policy_with_open(
        "[[open.hosts]]\nmatch = \"lab-*\"\nmode = \"allow\"\nmax_access = \"full\"\n",
    );
    for access in [Access::ReadOnly, Access::Full] {
        assert_eq!(
            policy.evaluate_open(open_request("lab-1", &[], access)),
            Decision::Allow
        );
    }
    assert_eq!(
        policy.evaluate_open(open_request("prod-1", &[], Access::ReadOnly)),
        Decision::Ask
    );
}

#[test]
fn max_access_defaults_to_read_only_and_more_is_asked_not_lowered() {
    let policy = policy_with_open("[[open.hosts]]\nmatch = \"lab-*\"\nmode = \"allow\"\n");
    assert_eq!(
        policy.evaluate_open(open_request("lab-1", &[], Access::ReadOnly)),
        Decision::Allow
    );
    assert_eq!(
        policy.evaluate_open(open_request("lab-1", &[], Access::Full)),
        Decision::Ask
    );
}

#[test]
fn root_is_asked_about_unless_the_rule_allows_it() {
    let without = policy_with_open(
        "[[open.hosts]]\nmatch = \"lab-*\"\nmode = \"allow\"\nmax_access = \"full\"\n",
    );
    let with = policy_with_open(
        "[[open.hosts]]\nmatch = \"lab-*\"\nmode = \"allow\"\nmax_access = \"full\"\n\
         allow_root = true\n",
    );
    let request = OpenRequest {
        root: true,
        ..open_request("lab-1", &[], Access::Full)
    };
    assert_eq!(without.evaluate_open(request), Decision::Ask);
    assert_eq!(with.evaluate_open(request), Decision::Allow);
}

#[test]
fn a_deny_rule_refuses_and_names_the_server() {
    let policy = policy_with_open("[[open.hosts]]\nmatch = \"prod-*\"\nmode = \"deny\"\n");
    let decision = policy.evaluate_open(open_request("prod-1", &[], Access::ReadOnly));
    assert!(deny_message(&decision).contains("prod-1"));
}

#[test]
fn an_ask_rule_asks_even_when_a_later_rule_would_allow() {
    let policy = policy_with_open(
        "[[open.hosts]]\nmatch = \"lab-1\"\nmode = \"ask\"\n\
         [[open.hosts]]\nmatch = \"lab-*\"\nmode = \"allow\"\n",
    );
    assert_eq!(
        policy.evaluate_open(open_request("lab-1", &[], Access::ReadOnly)),
        Decision::Ask
    );
    assert_eq!(
        policy.evaluate_open(open_request("lab-2", &[], Access::ReadOnly)),
        Decision::Allow
    );
}

#[test]
fn the_first_matching_open_rule_wins() {
    let policy = policy_with_open(
        "[[open.hosts]]\nmatch = \"lab-*\"\nmode = \"deny\"\n\
         [[open.hosts]]\nmatch = \"lab-1\"\nmode = \"allow\"\n",
    );
    assert!(matches!(
        policy.evaluate_open(open_request("lab-1", &[], Access::ReadOnly)),
        Decision::Deny(_)
    ));
}

#[test]
fn an_open_rule_with_a_tag_only_covers_servers_that_have_it() {
    let policy =
        policy_with_open("[[open.hosts]]\nmatch = \"*\"\ntag = \"lab\"\nmode = \"allow\"\n");
    assert_eq!(
        policy.evaluate_open(open_request(
            "web-1",
            &tags(&["lab", "web"]),
            Access::ReadOnly
        )),
        Decision::Allow
    );
    assert_eq!(
        policy.evaluate_open(open_request("web-2", &tags(&["prod"]), Access::ReadOnly)),
        Decision::Ask
    );
    assert_eq!(
        policy.evaluate_open(open_request("web-3", &[], Access::ReadOnly)),
        Decision::Ask
    );
}

#[test]
fn open_rules_match_aliases_without_regard_to_case() {
    let policy = policy_with_open("[[open.hosts]]\nmatch = \"LAB-*\"\nmode = \"allow\"\n");
    assert_eq!(
        policy.evaluate_open(open_request("lab-1", &[], Access::ReadOnly)),
        Decision::Allow
    );
}

#[test]
fn open_limits_default_and_can_be_lowered_or_raised_within_the_caps() {
    assert_eq!(
        parse(MINIMAL_APPROVE).open_limits(),
        OpenLimits {
            per_host: OPEN_DEFAULT_MAX_PER_HOST,
            per_agent: OPEN_DEFAULT_MAX_PER_AGENT,
        }
    );
    let policy =
        policy_with_open("[open]\nmax_sessions_per_host = 1\nmax_sessions_per_agent = 16\n");
    assert_eq!(
        policy.open_limits(),
        OpenLimits {
            per_host: 1,
            per_agent: OPEN_MAX_PER_AGENT,
        }
    );
}

#[test]
fn open_limits_outside_the_caps_are_rejected() {
    for open in [
        "[open]\nmax_sessions_per_host = 0\n".to_owned(),
        format!(
            "[open]\nmax_sessions_per_host = {}\n",
            OPEN_MAX_PER_HOST + 1
        ),
        "[open]\nmax_sessions_per_agent = 0\n".to_owned(),
        format!(
            "[open]\nmax_sessions_per_agent = {}\n",
            OPEN_MAX_PER_AGENT + 1
        ),
    ] {
        assert!(matches!(
            parse_err(&format!("{MINIMAL_APPROVE}\n{open}")),
            PolicyError::Invalid(_)
        ));
    }
}

#[test]
fn malformed_open_tables_are_rejected() {
    for open in [
        "[open]\ntypo = 1\n",
        "[[open.hosts]]\nmatch = \"lab-*\"\n",
        "[[open.hosts]]\nmatch = \"lab-*\"\nmode = \"maybe\"\n",
        "[[open.hosts]]\nmatch = \"lab-*\"\nmode = \"allow\"\nmax_access = \"root\"\n",
        "[[open.hosts]]\nmatch = \"lab-*\"\nmode = \"allow\"\nsurprise = true\n",
    ] {
        assert!(matches!(
            parse_err(&format!("{MINIMAL_APPROVE}\n{open}")),
            PolicyError::Parse(_)
        ));
    }
    for open in [
        "[[open.hosts]]\nmatch = \" \"\nmode = \"allow\"\n",
        "[[open.hosts]]\nmatch = \"lab-*\"\ntag = \"\"\nmode = \"allow\"\n",
    ] {
        assert!(matches!(
            parse_err(&format!("{MINIMAL_APPROVE}\n{open}")),
            PolicyError::Invalid(_)
        ));
    }
}

#[test]
fn open_rules_do_not_change_how_writes_are_decided() {
    let policy = policy_with_open("[[open.hosts]]\nmatch = \"*\"\nmode = \"allow\"\n");
    assert_eq!(
        decide(&policy, "lab-1", PolicyRequest::Exec("id")),
        Decision::Ask
    );
}
