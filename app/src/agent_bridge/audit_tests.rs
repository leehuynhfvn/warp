use super::*;

fn record(command: &str) -> AuditRecord {
    AuditRecord {
        ts_unix: 1_790_000_000,
        request_id: Uuid::nil(),
        agent: Some("claude-code".to_owned()),
        action: "remote.exec",
        session_id: "12".to_owned(),
        host: "prod-1".to_owned(),
        user: "root".to_owned(),
        cwd: Some("/etc".to_owned()),
        command: Some(command.to_owned()),
        path: None,
        exit_code: Some(0),
        result: AuditOutcome::Ok,
        error_code: None,
        policy_decision: None,
        policy_reason: None,
        agent_id: None,
        duration_ms: 42,
        bytes: None,
        still_running: false,
    }
}

fn lines(path: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("every line is JSON"))
        .collect()
}

#[test]
fn each_record_is_one_json_line_without_absent_fields() {
    let dir = tempfile::tempdir().unwrap();
    append(dir.path(), &record("nginx -t")).unwrap();
    append(dir.path(), &record("echo\nsecond line")).unwrap();

    let records = lines(&dir.path().join(AUDIT_FILE_NAME));
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["action"], "remote.exec");
    assert_eq!(records[0]["agent"], "claude-code");
    assert_eq!(records[0]["command"], "nginx -t");
    assert_eq!(records[0]["result"], "ok");
    assert_eq!(records[0]["duration_ms"], 42);
    assert!(records[0].get("path").is_none());
    assert!(records[0].get("error_code").is_none());
    assert_eq!(records[1]["command"], "echo\nsecond line");
}

#[test]
fn a_failed_request_records_its_error_code() {
    let dir = tempfile::tempdir().unwrap();
    let mut failed = record("false");
    failed.result = AuditOutcome::Error;
    failed.error_code = Some("session_busy".to_owned());
    failed.exit_code = None;
    append(dir.path(), &failed).unwrap();

    let records = lines(&dir.path().join(AUDIT_FILE_NAME));
    assert_eq!(records[0]["result"], "error");
    assert_eq!(records[0]["error_code"], "session_busy");
    assert!(records[0].get("exit_code").is_none());
}

#[test]
fn the_directory_is_created_with_its_parents() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".warp").join("agent-bridge");
    append(&dir, &record("ls")).unwrap();
    assert!(dir.join(AUDIT_FILE_NAME).is_file());
}

#[cfg(unix)]
#[test]
fn the_log_and_its_directory_are_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("a").join("b");
    append(&dir, &record("ls")).unwrap();

    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join(AUDIT_FILE_NAME)), 0o600);
}

#[test]
fn a_full_log_is_rotated_and_the_previous_rotation_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(AUDIT_FILE_NAME);
    let rotated = dir.path().join(ROTATED_FILE_NAME);
    let one_record = {
        append_with_limit(dir.path(), &record("first"), u64::MAX).unwrap();
        fs::metadata(&path).unwrap().len()
    };

    append_with_limit(dir.path(), &record("second"), one_record).unwrap();
    assert_eq!(lines(&rotated)[0]["command"], "first");
    assert_eq!(lines(&path).len(), 1);
    assert_eq!(lines(&path)[0]["command"], "second");

    append_with_limit(dir.path(), &record("third"), one_record).unwrap();
    assert_eq!(lines(&rotated)[0]["command"], "second");
    assert_eq!(lines(&path)[0]["command"], "third");
}

#[test]
fn a_log_below_the_limit_is_appended_to() {
    let dir = tempfile::tempdir().unwrap();
    append_with_limit(dir.path(), &record("a"), 1 << 20).unwrap();
    append_with_limit(dir.path(), &record("b"), 1 << 20).unwrap();
    assert_eq!(lines(&dir.path().join(AUDIT_FILE_NAME)).len(), 2);
    assert!(!dir.path().join(ROTATED_FILE_NAME).exists());
}

#[test]
fn an_unwritable_location_is_reported_not_swallowed() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("file");
    fs::write(&blocker, "").unwrap();
    let error = append(&blocker.join("agent-bridge"), &record("ls")).unwrap_err();
    assert!(matches!(error, AgentBridgeError::Io(_)));
}
