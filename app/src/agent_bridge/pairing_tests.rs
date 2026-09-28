use chrono::TimeZone as _;

use super::*;

fn agent(id: &str, token_sha256: &str) -> PairedAgent {
    PairedAgent {
        id: id.to_owned(),
        token_sha256: token_sha256.to_owned(),
        paired_at: Utc.with_ymd_and_hms(2026, 9, 28, 9, 0, 0).unwrap(),
    }
}

#[test]
fn hash_is_lowercase_hex_sha256() {
    let digest = hash("hello");
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
    // Known SHA-256 of "hello".
    assert_eq!(
        digest,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn parse_and_serialize_round_trip() {
    let agents = vec![agent("claude-code", "a".repeat(64).as_str())];
    let text = serialize(&agents).unwrap();
    assert_eq!(parse(&text).unwrap(), agents);
}

#[test]
fn parse_rejects_unknown_fields() {
    let error = parse("[[agents]]\nid = \"x\"\ntoken_sha256 = \"y\"\npaired_at = \"2026-09-28T09:00:00Z\"\nextra = true\n")
        .unwrap_err();
    assert!(matches!(error, PairingError::Parse(_)));
}

#[test]
fn parse_with_no_agents_key_is_an_empty_list() {
    assert_eq!(parse("").unwrap(), Vec::new());
}

#[test]
fn find_matches_by_token_hash() {
    let claude = agent("claude-code", "a".repeat(64).as_str());
    let codex = agent("codex", "b".repeat(64).as_str());
    let agents = vec![claude.clone(), codex.clone()];
    assert_eq!(find(&agents, &"a".repeat(64)), Some(&claude));
    assert_eq!(find(&agents, &"b".repeat(64)), Some(&codex));
    assert_eq!(find(&agents, &"c".repeat(64)), None);
}

#[test]
fn next_id_returns_the_name_when_it_is_free() {
    assert_eq!(next_id(&[], "claude-code"), "claude-code");
}

#[test]
fn next_id_appends_a_suffix_for_a_duplicate_name() {
    let existing = vec![
        agent("claude-code", &"a".repeat(64)),
        agent("claude-code-2", &"b".repeat(64)),
    ];
    assert_eq!(next_id(&existing, "claude-code"), "claude-code-3");
}

#[test]
fn load_with_no_file_is_empty() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(load(home.path()), Vec::new());
}

#[test]
fn load_with_a_corrupt_file_is_empty_and_does_not_panic() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".warp").join("agent-ops");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("agents.toml"), "not valid toml {{{").unwrap();
    assert_eq!(load(home.path()), Vec::new());
}

#[test]
fn add_creates_the_file_and_returns_the_new_entry() {
    let home = tempfile::tempdir().unwrap();
    let added = add(home.path(), "claude-code", "a".repeat(64)).unwrap();
    assert_eq!(added.id, "claude-code");
    assert_eq!(added.token_sha256, "a".repeat(64));
    assert_eq!(load(home.path()), vec![added]);
}

#[test]
fn add_appends_to_existing_agents_and_disambiguates_the_name() {
    let home = tempfile::tempdir().unwrap();
    add(home.path(), "claude-code", "a".repeat(64)).unwrap();
    let second = add(home.path(), "claude-code", "b".repeat(64)).unwrap();
    assert_eq!(second.id, "claude-code-2");
    assert_eq!(load(home.path()).len(), 2);
}

#[test]
fn forget_all_empties_the_file() {
    let home = tempfile::tempdir().unwrap();
    add(home.path(), "claude-code", "a".repeat(64)).unwrap();
    forget_all(home.path()).unwrap();
    assert_eq!(load(home.path()), Vec::new());
}

#[cfg(unix)]
#[test]
fn add_writes_the_file_with_0600_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let home = tempfile::tempdir().unwrap();
    add(home.path(), "claude-code", "a".repeat(64)).unwrap();
    let path = home.path().join(".warp").join("agent-ops").join("agents.toml");
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn add_leaves_no_leftover_temp_file() {
    let home = tempfile::tempdir().unwrap();
    add(home.path(), "claude-code", "a".repeat(64)).unwrap();
    let dir = home.path().join(".warp").join("agent-ops");
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["agents.toml".to_owned()]);
}
