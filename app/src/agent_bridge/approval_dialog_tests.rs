use std::time::{Duration, SystemTime};

use warpui::WindowId;

use super::*;
use crate::terminal::model::session::SessionId;

fn command_request(command: &str, cwd: Option<&str>, visible: bool) -> ApprovalRequest {
    ApprovalRequest {
        request_id: Uuid::new_v4(),
        session: SessionId::from(1u64),
        session_label: "root@lab-1".to_owned(),
        agent: AgentLabel {
            claimed: Some("claude-code".to_owned()),
            agent_id: None,
        },
        subject: ApprovalSubject::Command {
            command: command.to_owned(),
            cwd: cwd.map(str::to_owned),
            visible,
        },
        deadline: SystemTime::now() + Duration::from_secs(300),
        window_id: WindowId::from_usize(1),
    }
}

fn write_request(
    bytes: u64,
    creates: bool,
    preview: &str,
    truncated_lines: usize,
) -> ApprovalRequest {
    ApprovalRequest {
        request_id: Uuid::new_v4(),
        session: SessionId::from(1u64),
        session_label: "root@lab-1".to_owned(),
        agent: AgentLabel {
            claimed: Some("claude-code".to_owned()),
            agent_id: None,
        },
        subject: ApprovalSubject::Write {
            path: "/etc/nginx/nginx.conf".to_owned(),
            bytes,
            creates,
            preview: preview.to_owned(),
            preview_truncated_lines: truncated_lines,
        },
        deadline: SystemTime::now() + Duration::from_secs(300),
        window_id: WindowId::from_usize(1),
    }
}

#[test]
fn a_hidden_command_says_it_runs_in_the_background() {
    let request = command_request("uptime", None, false);
    let (title, body) = content(&request);
    assert_eq!(title, "Run on root@lab-1?");
    assert!(body.contains("Runs in the background"));
    assert!(body.contains("uptime"));
    assert!(!body.contains("Directory:"));
}

#[test]
fn a_visible_command_says_it_runs_in_the_terminal() {
    let request = command_request("df -h", None, true);
    let (_, body) = content(&request);
    assert!(body.contains("Runs visibly in the terminal"));
}

#[test]
fn a_directory_is_shown_when_the_request_has_one() {
    let request = command_request("ls", Some("/etc/nginx"), false);
    let (_, body) = content(&request);
    assert!(body.contains("Directory: /etc/nginx"));
}

#[test]
fn control_characters_in_the_command_are_stripped() {
    let request = command_request("echo \u{1b}[31mhi\u{1b}[0m", None, false);
    let (_, body) = content(&request);
    assert!(!body.contains('\u{1b}'), "{body:?}");
}

#[test]
fn a_long_command_is_shown_in_full() {
    let long_command = "echo ".to_owned() + &"x".repeat(500);
    let request = command_request(&long_command, None, false);
    let (_, body) = content(&request);
    assert!(body.contains(&long_command));
}

#[test]
fn a_write_that_creates_a_new_file_says_so() {
    let request = write_request(128, true, "server {\n  listen 80;\n}", 0);
    let (title, body) = content(&request);
    assert_eq!(title, "Write a file on root@lab-1?");
    assert!(body.contains("Path: /etc/nginx/nginx.conf"));
    assert!(body.contains("Size: 128 bytes"));
    assert!(body.contains("Creates a new file"));
    assert!(!body.contains("more lines"));
}

#[test]
fn a_write_that_replaces_a_file_mentions_the_backup() {
    let request = write_request(64, false, "listen 80;", 0);
    let (_, body) = content(&request);
    assert!(body.contains("Replaces the existing file (a backup is kept on the server)"));
}

#[test]
fn a_truncated_write_preview_notes_how_many_more_lines() {
    let request = write_request(4000, false, "line 1\nline 2", 37);
    let (_, body) = content(&request);
    assert!(body.contains("… 37 more lines"));
}

#[test]
fn a_write_preview_with_nothing_cut_mentions_no_more_lines() {
    let request = write_request(10, true, "hello", 0);
    let (_, body) = content(&request);
    assert!(!body.contains("more lines"));
}

#[test]
fn an_agent_that_gave_no_name_is_labeled_generically() {
    let mut request = command_request("uptime", None, false);
    request.agent = AgentLabel::default();
    let (_, body) = content(&request);
    assert!(body.contains("Agent: an agent that did not give its name"));
}

#[test]
fn a_claimed_agent_name_is_marked_unverified_when_not_paired() {
    let request = command_request("uptime", None, false);
    let (_, body) = content(&request);
    assert!(body.contains("Agent: claude-code (unverified name)"));
}

#[test]
fn a_paired_agent_is_shown_by_its_paired_id_instead_of_its_claimed_name() {
    let mut request = command_request("uptime", None, false);
    request.agent = AgentLabel {
        claimed: Some("some-other-claimed-name".to_owned()),
        agent_id: Some("claude-code".to_owned()),
    };
    let (_, body) = content(&request);
    assert!(body.contains("Agent: claude-code (paired)"));
    assert!(!body.contains("some-other-claimed-name"));
}

#[test]
fn the_deadline_is_shown_as_a_clock_time() {
    let request = command_request("uptime", None, false);
    let (_, body) = content(&request);
    let line = body
        .lines()
        .find(|line| line.starts_with("Denied automatically at"))
        .expect("a deadline line");
    assert!(line.ends_with("if no one answers."), "{line:?}");
    // "HH:MM" sits right after "Denied automatically at ".
    let clock = &line["Denied automatically at ".len()..][..5];
    assert_eq!(clock.as_bytes()[2], b':', "{clock:?}");
}
