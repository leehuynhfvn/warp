use std::fs;
use std::path::Path;

use super::*;

fn request(action: ActionKind) -> OpenRequest<'static> {
    OpenRequest {
        action,
        request_id: Uuid::nil(),
        agent: Some("claude-code"),
        agent_id: Some("claude-code"),
        alias: "lab-1",
        purpose: Some("check the disk"),
        access: Some(Access::Full),
    }
}

fn lines(dir: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(dir.join("audit.jsonl"))
        .expect("the log exists")
        .lines()
        .map(|line| serde_json::from_str(line).expect("every line is JSON"))
        .collect()
}

#[test]
fn an_open_request_leaves_a_started_line_and_a_closing_line() {
    let dir = tempfile::tempdir().unwrap();
    let audit = SessionRequestAudit::begin(
        Some(dir.path().to_owned()),
        request(ActionKind::RemoteSessionOpen),
    )
    .with_policy("approved", None);
    audit.record_approval_requested().unwrap();
    audit.record_start().unwrap();
    audit.with_session("12", "ops").finish(Ok(()));

    let lines = lines(dir.path());
    let results: Vec<_> = lines.iter().map(|line| line["result"].as_str()).collect();
    assert_eq!(
        results,
        [Some("approval_requested"), Some("started"), Some("ok")]
    );
    for line in &lines {
        assert_eq!(line["action"], "remote.session.open");
        assert_eq!(line["host"], "lab-1");
        assert_eq!(line["agent"], "claude-code");
        assert_eq!(line["agent_id"], "claude-code");
        assert_eq!(line["purpose"], "check the disk");
        assert_eq!(line["access"], "full");
        assert_eq!(line["policy_decision"], "approved");
        assert_eq!(line["request_id"], Uuid::nil().to_string());
    }
    assert_eq!(lines[0]["session_id"], "");
    assert_eq!(lines[2]["session_id"], "12");
    assert_eq!(lines[2]["user"], "ops");
}

#[test]
fn a_denied_request_records_why() {
    let dir = tempfile::tempdir().unwrap();
    let error = AgentBridgeError::PolicyDenied("the user denied it.".to_owned());
    SessionRequestAudit::begin(
        Some(dir.path().to_owned()),
        request(ActionKind::RemoteSessionOpen),
    )
    .with_policy("ask_denied", None)
    .finish(Err(&error));

    let lines = lines(dir.path());
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["result"], "error");
    assert_eq!(lines[0]["error_code"], "policy_denied");
    assert_eq!(lines[0]["policy_decision"], "ask_denied");
    assert_eq!(lines[0]["policy_reason"], "the user denied it.");
}

#[test]
fn a_close_line_has_no_purpose_and_no_access() {
    let dir = tempfile::tempdir().unwrap();
    let close = OpenRequest {
        purpose: None,
        access: None,
        ..request(ActionKind::RemoteSessionClose)
    };
    SessionRequestAudit::begin(Some(dir.path().to_owned()), close)
        .with_session("12", "ops")
        .finish(Ok(()));

    let lines = lines(dir.path());
    assert_eq!(lines[0]["action"], "remote.session.close");
    assert!(lines[0].get("purpose").is_none());
    assert!(lines[0].get("access").is_none());
}

#[test]
fn an_agent_name_that_is_not_a_valid_label_is_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let request = OpenRequest {
        agent: Some("evil\nname"),
        ..request(ActionKind::RemoteSessionOpen)
    };
    SessionRequestAudit::begin(Some(dir.path().to_owned()), request).finish(Ok(()));
    assert!(lines(dir.path())[0].get("agent").is_none());
}

#[test]
fn a_log_that_cannot_be_written_stops_the_request_before_it_starts() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("not-a-directory");
    fs::write(&blocked, "").unwrap();
    let audit = SessionRequestAudit::begin(Some(blocked), request(ActionKind::RemoteSessionOpen));
    assert!(matches!(
        audit.record_start(),
        Err(AgentBridgeError::Io(message)) if message.contains("was not run")
    ));
    assert!(matches!(
        audit.record_approval_requested(),
        Err(AgentBridgeError::Io(message)) if message.contains("not queued")
    ));
}

#[test]
fn without_a_log_directory_nothing_is_written_and_nothing_fails() {
    let audit = SessionRequestAudit::begin(None, request(ActionKind::RemoteSessionOpen));
    assert!(audit.record_start().is_ok());
    assert!(audit.record_approval_requested().is_ok());
    audit.finish(Ok(()));
}
