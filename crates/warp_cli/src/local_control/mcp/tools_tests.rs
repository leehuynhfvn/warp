use std::collections::VecDeque;

use local_control::protocol::{
    ErrorCode, RemoteAccess, RemoteAttachment, RemoteCommandBlock, RemoteSessionKind,
    RemoteSessionSummary, RemoteStream,
};

use super::*;

/// Answers calls from a script and records them.
#[derive(Default)]
struct FakeTransport {
    answers: VecDeque<(ActionKind, Result<Value, ControlError>)>,
    calls: Vec<Call>,
}

#[derive(Debug, Clone)]
struct Call {
    params: Value,
    session: Option<String>,
    timeout: Duration,
}

impl ControlTransport for FakeTransport {
    fn call(
        &mut self,
        action: ActionKind,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, ControlError> {
        self.calls.push(Call {
            params,
            session: session.map(str::to_owned),
            timeout,
        });
        let (expected, answer) = self
            .answers
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected call to {action:?}"));
        assert_eq!(action, expected, "calls came in another order");
        answer
    }
}

fn tools() -> Tools<FakeTransport> {
    Tools::new(FakeTransport::default(), Redactor::with_default_patterns())
}

fn answer(tools: &mut Tools<FakeTransport>, action: ActionKind, data: impl serde::Serialize) {
    let data = serde_json::to_value(data).expect("answer encodes");
    tools.transport.answers.push_back((action, Ok(data)));
}

fn fail(tools: &mut Tools<FakeTransport>, action: ActionKind, error: ControlError) {
    tools.transport.answers.push_back((action, Err(error)));
}

fn call(tools: &mut Tools<FakeTransport>, name: &str, arguments: Value) -> ToolResult {
    let result = tools
        .call_tool(name, arguments)
        .unwrap_or_else(|| panic!("{name} is a tool"));
    assert!(
        tools.transport.answers.is_empty(),
        "unused answers: {:?}",
        tools.transport.answers
    );
    result
}

fn session_ref() -> RemoteSessionRef {
    RemoteSessionRef {
        session_id: "Pane 7".to_owned(),
        host: "prod-1".to_owned(),
        user: "root".to_owned(),
    }
}

fn summary(session_id: &str, attached: bool) -> RemoteSessionSummary {
    RemoteSessionSummary {
        session_id: session_id.to_owned(),
        window_index: 0,
        tab_index: 0,
        pane_index: 0,
        is_active: false,
        session_type: RemoteSessionKind::Remote,
        host: "prod-1".to_owned(),
        user: "root".to_owned(),
        shell: "bash".to_owned(),
        cwd: Some("/root".to_owned()),
        attached: attached.then_some(RemoteAttachment {
            access: RemoteAccess::Full,
            idle_secs: 0,
            expires_in_secs: 1800,
            exec_count: 0,
        }),
    }
}

fn session_list(sessions: &[(&str, bool)]) -> RemoteSessionListResult {
    RemoteSessionListResult {
        sessions: sessions
            .iter()
            .map(|(id, attached)| summary(id, *attached))
            .collect(),
    }
}

fn file(path: &str, content: &str) -> RemoteFileReadResult {
    RemoteFileReadResult::Ok {
        session: session_ref(),
        path: path.to_owned(),
        size: content.len() as u64,
        sha256: sha256_hex(content.as_bytes()),
        content_base64: BASE64.encode(content),
    }
}

fn not_found(path: &str) -> RemoteFileReadResult {
    RemoteFileReadResult::NotFound {
        session: session_ref(),
        path: path.to_owned(),
    }
}

fn written(path: &str, content: &str, created: bool) -> RemoteFileWriteResult {
    RemoteFileWriteResult {
        session: session_ref(),
        path: path.to_owned(),
        bytes: content.len() as u64,
        sha256: sha256_hex(content.as_bytes()),
        backup_path: (!created).then(|| "/root/.warp-agent/backups/a.conf.1".to_owned()),
        created,
    }
}

fn write_params(call: &Call) -> RemoteFileWriteParams {
    serde_json::from_value(call.params.clone()).expect("write params decode")
}

fn read_first(tools: &mut Tools<FakeTransport>, path: &str, content: &str) {
    answer(tools, ActionKind::RemoteFileRead, file(path, content));
    let result = call(
        tools,
        "read_file",
        json!({ "path": path, "session_id": "Pane 7" }),
    );
    assert!(!result.is_error, "{}", result.text);
}

#[test]
fn the_only_attached_session_is_used_when_none_is_given() {
    let mut tools = tools();
    answer(
        &mut tools,
        ActionKind::RemoteSessionList,
        session_list(&[("Pane 1", false), ("Pane 7", true)]),
    );
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", "x\n"),
    );

    let result = call(&mut tools, "read_file", json!({ "path": "/etc/a" }));

    assert!(!result.is_error, "{}", result.text);
    assert_eq!(tools.transport.calls[0].session, None);
    assert_eq!(tools.transport.calls[1].session.as_deref(), Some("Pane 7"));
}

#[test]
fn no_or_several_attached_sessions_need_the_user_or_a_session_id() {
    let mut tools = tools();
    answer(
        &mut tools,
        ActionKind::RemoteSessionList,
        session_list(&[("Pane 1", false)]),
    );
    let result = call(&mut tools, "exec", json!({ "command": "id" }));
    assert!(result.is_error);
    assert!(
        result.text.starts_with("No session is attached."),
        "{}",
        result.text
    );
    assert!(result.text.contains("\"Pane 1\""));

    answer(
        &mut tools,
        ActionKind::RemoteSessionList,
        session_list(&[("Pane 1", true), ("Pane 2", true)]),
    );
    let result = call(&mut tools, "exec", json!({ "command": "id" }));
    assert!(result.is_error);
    assert!(
        result.text.starts_with("2 sessions are attached"),
        "{}",
        result.text
    );
}

#[test]
fn exec_sends_the_command_and_renders_the_result() {
    let mut tools = tools();
    tools.set_client_name("claude code");
    answer(
        &mut tools,
        ActionKind::RemoteExec,
        RemoteExecResult {
            session: session_ref(),
            cwd: Some("/etc".to_owned()),
            exit_code: 0,
            timed_out: false,
            duration_ms: 40,
            stdout: RemoteStream {
                text: "root\n".to_owned(),
                total_bytes: 5,
                truncated: false,
            },
            stderr: RemoteStream {
                text: String::new(),
                total_bytes: 0,
                truncated: false,
            },
        },
    );

    let result = call(
        &mut tools,
        "exec",
        json!({ "command": "id -un", "timeout_secs": 5, "session_id": "Pane 7" }),
    );

    assert_eq!(
        result,
        ToolResult::ok("exit_code: 0 (40ms) root@prod-1:/etc\n--- stdout ---\nroot\n")
    );
    let sent = &tools.transport.calls[0];
    assert_eq!(
        sent.params,
        json!({ "command": "id -un", "timeout_secs": 5, "agent": "claude-code" })
    );
    assert_eq!(sent.timeout, Duration::from_secs(5) + EXEC_CLIENT_MARGIN);
}

#[test]
fn read_file_numbers_lines_and_hides_secrets() {
    let mut tools = tools();
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/app.env", "HOST=db\nDB_PASSWORD=hunter2\n"),
    );

    let result = call(
        &mut tools,
        "read_file",
        json!({ "path": "app.env", "session_id": "Pane 7" }),
    );

    assert_eq!(
        result,
        ToolResult::ok(
            "root@prod-1:/etc/app.env (2 lines, 28 bytes)\n     1\tHOST=db\n     2\tDB_PASSWORD=*******\n"
        )
    );
}

#[test]
fn read_file_reports_missing_and_binary_files() {
    let mut tools = tools();
    answer(&mut tools, ActionKind::RemoteFileRead, not_found("/nope"));
    let result = call(
        &mut tools,
        "read_file",
        json!({ "path": "/nope", "session_id": "Pane 7" }),
    );
    assert_eq!(result, ToolResult::error("/nope does not exist."));

    let mut binary = file("/bin/x", "");
    if let RemoteFileReadResult::Ok { content_base64, .. } = &mut binary {
        *content_base64 = BASE64.encode([0xff, 0xfe, 0x00]);
    }
    answer(&mut tools, ActionKind::RemoteFileRead, binary);
    let result = call(
        &mut tools,
        "read_file",
        json!({ "path": "/bin/x", "session_id": "Pane 7" }),
    );
    assert!(result.is_error);
    assert!(result.text.contains("binary file"), "{}", result.text);
}

#[test]
fn read_file_shows_at_least_one_line() {
    let mut tools = tools();
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", "x\ny\n"),
    );

    let result = call(
        &mut tools,
        "read_file",
        json!({ "path": "/etc/a", "limit": 0, "session_id": "Pane 7" }),
    );

    assert!(result.text.contains("     1\tx\n"), "{}", result.text);
    assert!(result.text.contains("offset=2"), "{}", result.text);
}

#[test]
fn write_file_does_not_replace_binary_files() {
    let mut tools = tools();
    let mut binary = file("/bin/x", "");
    if let RemoteFileReadResult::Ok { content_base64, .. } = &mut binary {
        *content_base64 = BASE64.encode([0xff, 0xfe, 0x00]);
    }
    answer(&mut tools, ActionKind::RemoteFileRead, binary);

    let result = call(
        &mut tools,
        "write_file",
        json!({ "path": "/bin/x", "content": "text", "session_id": "Pane 7" }),
    );

    assert_eq!(
        result,
        ToolResult::error(
            "root@prod-1:/bin/x is a binary file; write_file only replaces text files."
        )
    );
}

#[test]
fn write_file_creates_a_missing_file() {
    let mut tools = tools();
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        not_found("/root/new"),
    );
    answer(
        &mut tools,
        ActionKind::RemoteFileWrite,
        written("/root/new", "hello\n", true),
    );

    let result = call(
        &mut tools,
        "write_file",
        json!({ "path": "/root/new", "content": "hello\n", "session_id": "Pane 7" }),
    );

    assert_eq!(
        result,
        ToolResult::ok("Created root@prod-1:/root/new (6 bytes).")
    );
    let params = write_params(&tools.transport.calls[1]);
    assert_eq!(params.expectation, WriteExpectation::MustNotExist);
    assert_eq!(params.content_base64, BASE64.encode("hello\n"));
}

#[test]
fn write_file_needs_a_read_first() {
    let mut tools = tools();
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", "old\n"),
    );

    let result = call(
        &mut tools,
        "write_file",
        json!({ "path": "/etc/a", "content": "new\n", "session_id": "Pane 7" }),
    );

    assert_eq!(
        result,
        ToolResult::error("Read root@prod-1:/etc/a with read_file before changing it.")
    );
}

#[test]
fn write_file_refuses_a_file_that_changed_since_it_was_read() {
    let mut tools = tools();
    read_first(&mut tools, "/etc/a", "old\n");
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", "changed\n"),
    );

    let result = call(
        &mut tools,
        "write_file",
        json!({ "path": "/etc/a", "content": "new\n", "session_id": "Pane 7" }),
    );

    assert!(result.is_error);
    assert!(
        result.text.contains("changed on the server"),
        "{}",
        result.text
    );
}

#[test]
fn write_file_refuses_to_rewrite_a_file_with_secrets() {
    let mut tools = tools();
    read_first(&mut tools, "/etc/a", "token=abc\n");
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", "token=abc\n"),
    );

    let result = call(
        &mut tools,
        "write_file",
        json!({ "path": "/etc/a", "content": "token=***\n", "session_id": "Pane 7" }),
    );

    assert!(result.is_error);
    assert!(result.text.contains("edit_file"), "{}", result.text);
}

#[test]
fn write_file_overwrites_the_version_that_was_read() {
    let mut tools = tools();
    read_first(&mut tools, "/etc/a", "old\n");
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", "old\n"),
    );
    answer(
        &mut tools,
        ActionKind::RemoteFileWrite,
        written("/etc/a", "new\n", false),
    );

    let result = call(
        &mut tools,
        "write_file",
        json!({ "path": "/etc/a", "content": "new\n", "session_id": "Pane 7" }),
    );

    assert!(!result.is_error, "{}", result.text);
    assert!(result.text.contains("/root/.warp-agent/backups/a.conf.1"));
    let params = write_params(&tools.transport.calls[2]);
    assert_eq!(
        params.expectation,
        WriteExpectation::MustMatch {
            sha256: sha256_hex(b"old\n")
        }
    );
}

#[test]
fn edit_file_sends_the_edited_content_for_the_version_read() {
    let mut tools = tools();
    let before = "events {\n    worker_connections 768;\n}\nAPI_KEY=s3cr3t\n";
    let after = "events {\n    worker_connections 2048;\n}\nAPI_KEY=s3cr3t\n";
    read_first(&mut tools, "/etc/nginx.conf", before);
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/nginx.conf", before),
    );
    answer(
        &mut tools,
        ActionKind::RemoteFileWrite,
        written("/etc/nginx.conf", after, false),
    );

    let result = call(
        &mut tools,
        "edit_file",
        json!({
            "path": "/etc/nginx.conf",
            "old_string": "worker_connections 768;",
            "new_string": "worker_connections 2048;",
            "session_id": "Pane 7",
        }),
    );

    assert!(!result.is_error, "{}", result.text);
    assert!(
        result
            .text
            .starts_with("Edited root@prod-1:/etc/nginx.conf: replaced 1 occurrence."),
        "{}",
        result.text
    );
    assert!(
        result.text.contains("     4\tAPI_KEY=******\n"),
        "{}",
        result.text
    );
    assert!(!result.text.contains("s3cr3t"));
    let params = write_params(&tools.transport.calls[2]);
    assert_eq!(
        params.expectation,
        WriteExpectation::MustMatch {
            sha256: sha256_hex(before.as_bytes())
        }
    );
    assert_eq!(params.content_base64, BASE64.encode(after));

    // The written version counts as seen, so a second edit needs no new read_file.
    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/nginx.conf", after),
    );
    answer(
        &mut tools,
        ActionKind::RemoteFileWrite,
        written("/etc/nginx.conf", before, false),
    );
    let result = call(
        &mut tools,
        "edit_file",
        json!({
            "path": "/etc/nginx.conf",
            "old_string": "2048",
            "new_string": "768",
            "session_id": "Pane 7",
        }),
    );
    assert!(!result.is_error, "{}", result.text);
}

#[test]
fn edit_file_reports_missing_and_repeated_old_strings() {
    let mut tools = tools();
    read_first(&mut tools, "/etc/a", "x\nx\n");

    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", "x\nx\n"),
    );
    let result = call(
        &mut tools,
        "edit_file",
        json!({ "path": "/etc/a", "old_string": "y", "new_string": "z", "session_id": "Pane 7" }),
    );
    assert!(result.is_error);
    assert!(
        result.text.starts_with("old_string was not found"),
        "{}",
        result.text
    );

    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", "x\nx\n"),
    );
    let result = call(
        &mut tools,
        "edit_file",
        json!({ "path": "/etc/a", "old_string": "x", "new_string": "z", "session_id": "Pane 7" }),
    );
    assert!(result.is_error);
    assert!(
        result.text.starts_with("old_string occurs 2 times"),
        "{}",
        result.text
    );
}

#[test]
fn edit_file_cannot_match_or_copy_hidden_secrets() {
    let mut tools = tools();
    let content = "password=hunter22\nport=1\n";
    read_first(&mut tools, "/etc/a", content);

    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", content),
    );
    let result = call(
        &mut tools,
        "edit_file",
        json!({
            "path": "/etc/a",
            "old_string": "password=********",
            "new_string": "password=x",
            "session_id": "Pane 7",
        }),
    );
    assert!(result.is_error);
    assert!(result.text.contains("hidden as ****"), "{}", result.text);

    answer(
        &mut tools,
        ActionKind::RemoteFileRead,
        file("/etc/a", content),
    );
    let result = call(
        &mut tools,
        "edit_file",
        json!({
            "path": "/etc/a",
            "old_string": "port=1\n",
            "new_string": "port=1\npassword=********\n",
            "session_id": "Pane 7",
        }),
    );
    assert!(result.is_error);
    assert!(
        result.text.contains("stands for a hidden secret"),
        "{}",
        result.text
    );
}

#[test]
fn control_errors_are_tool_errors_with_the_message_of_the_app() {
    let mut tools = tools();
    fail(
        &mut tools,
        ActionKind::RemoteExec,
        ControlError::new(
            ErrorCode::SessionNotAttached,
            "Session Pane 7 (root@prod-1) is not attached.",
        ),
    );

    let result = call(
        &mut tools,
        "exec",
        json!({ "command": "id", "session_id": "Pane 7" }),
    );

    assert_eq!(
        result,
        ToolResult::error(
            "Error (session_not_attached): Session Pane 7 (root@prod-1) is not attached."
        )
    );
}

#[test]
fn invalid_arguments_and_unknown_tools() {
    let mut tools = tools();
    let result = call(&mut tools, "exec", json!({ "command": "id", "sudo": true }));
    assert!(result.is_error);
    assert!(
        result.text.starts_with("Invalid arguments for exec"),
        "{}",
        result.text
    );

    assert_eq!(tools.call_tool("run_script", json!({})), None);
}

#[test]
fn agent_names_are_made_acceptable_to_the_app() {
    assert_eq!(agent_name("claude-code"), "claude-code");
    assert_eq!(agent_name("Gemini CLI/1"), "Gemini-CLI-1");
    assert_eq!(agent_name(""), UNKNOWN_AGENT);
    assert_eq!(agent_name(&"a".repeat(100)).len(), MAX_AGENT_NAME_BYTES);
}

#[test]
fn every_tool_has_a_schema_and_the_list_matches_the_dispatch() {
    let definitions = tool_definitions();
    let names: Vec<&str> = definitions
        .as_array()
        .expect("tools are an array")
        .iter()
        .map(|tool| {
            assert_eq!(tool["inputSchema"]["type"], json!("object"));
            tool["name"].as_str().expect("tools have names")
        })
        .collect();
    assert_eq!(
        names,
        [
            "list_sessions",
            "exec",
            "exec_visible",
            "read_file",
            "recent_output",
            "write_file",
            "edit_file"
        ]
    );
}

fn recent(blocks: Vec<RemoteCommandBlock>) -> RemoteOutputRecentResult {
    RemoteOutputRecentResult {
        session: session_ref(),
        blocks,
    }
}

#[test]
fn recent_output_asks_for_a_count_in_range_and_hides_secrets() {
    let mut tools = tools();
    answer(
        &mut tools,
        ActionKind::RemoteOutputRecent,
        recent(vec![RemoteCommandBlock {
            command: "cat .env".to_owned(),
            exit_code: 0,
            cwd: Some("/srv/app".to_owned()),
            output: "AWS_SECRET_ACCESS_KEY=abcd1234efgh5678".to_owned(),
            output_rows: 1,
            truncated: false,
        }]),
    );

    let result = call(
        &mut tools,
        "recent_output",
        json!({ "count": 50, "session_id": "Pane 7" }),
    );

    let sent = &tools.transport.calls[0];
    assert_eq!(sent.params, json!({ "count": 10, "agent": "mcp-unknown" }));
    assert_eq!(sent.session.as_deref(), Some("Pane 7"));
    assert!(!result.is_error, "{}", result.text);
    let text = &result.text;
    assert!(text.contains("$ cat .env\n[exit 0, in /srv/app]"), "{text}");
    assert!(!text.contains("abcd1234efgh5678"), "{text}");
}

#[test]
fn recent_output_defaults_to_three_commands_of_the_attached_session() {
    let mut tools = tools();
    answer(
        &mut tools,
        ActionKind::RemoteSessionList,
        session_list(&[("Pane 7", true)]),
    );
    answer(
        &mut tools,
        ActionKind::RemoteOutputRecent,
        recent(Vec::new()),
    );

    let result = call(&mut tools, "recent_output", json!({}));

    assert_eq!(tools.transport.calls[1].params["count"], json!(3));
    assert_eq!(
        result,
        ToolResult::ok("No finished commands in root@prod-1 yet.")
    );
}

fn visible_result(
    exit_code: Option<i32>,
    alt_screen: bool,
    output: &str,
) -> RemoteExecVisibleResult {
    RemoteExecVisibleResult {
        session: session_ref(),
        command: "systemctl status nginx".to_owned(),
        cwd: Some("/root".to_owned()),
        exit_code,
        still_running: exit_code.is_none(),
        alt_screen,
        duration_ms: 1200,
        output: output.to_owned(),
        output_rows: 1,
        truncated: false,
    }
}

#[test]
fn exec_visible_sends_the_command_without_a_directory_and_renders_the_block() {
    let mut tools = tools();
    tools.set_client_name("claude code");
    answer(
        &mut tools,
        ActionKind::RemoteExecVisible,
        visible_result(Some(3), false, "inactive (dead)\n"),
    );

    let result = call(
        &mut tools,
        "exec_visible",
        json!({ "command": "systemctl status nginx", "timeout_secs": 30, "session_id": "Pane 7" }),
    );

    assert_eq!(
        result,
        ToolResult::ok("exit_code: 3 (1.2s) root@prod-1:/root\n--- output ---\ninactive (dead)\n")
    );
    let sent = &tools.transport.calls[0];
    assert_eq!(
        sent.params,
        json!({ "command": "systemctl status nginx", "timeout_secs": 30, "agent": "claude-code" })
    );
    assert_eq!(sent.session.as_deref(), Some("Pane 7"));
    assert_eq!(sent.timeout, Duration::from_secs(30) + EXEC_CLIENT_MARGIN);
}

#[test]
fn exec_visible_reports_a_command_that_is_still_running() {
    let mut tools = tools();
    answer(
        &mut tools,
        ActionKind::RemoteExecVisible,
        visible_result(None, true, "root:x:0:0\n"),
    );

    let result = call(
        &mut tools,
        "exec_visible",
        json!({ "command": "less /etc/passwd", "session_id": "Pane 7" }),
    );

    assert!(!result.is_error);
    assert!(
        result.text.starts_with("still running after 1.2s"),
        "{}",
        result.text
    );
    assert!(
        result.text.contains("full-screen program"),
        "{}",
        result.text
    );
    assert!(result.text.contains("recent_output"), "{}", result.text);
    assert!(
        result.text.contains("--- output so far ---\nroot:x:0:0\n"),
        "{}",
        result.text
    );
    assert_eq!(
        tools.transport.calls[0].timeout,
        Duration::from_secs(EXEC_DEFAULT_TIMEOUT_SECS.into()) + EXEC_CLIENT_MARGIN
    );
}

#[test]
fn exec_visible_passes_a_busy_shell_through_as_an_error() {
    let mut tools = tools();
    fail(
        &mut tools,
        ActionKind::RemoteExecVisible,
        ControlError::new(ErrorCode::SessionBusy, "The session's shell is busy."),
    );

    let result = call(
        &mut tools,
        "exec_visible",
        json!({ "command": "uptime", "session_id": "Pane 7" }),
    );

    assert_eq!(
        result,
        ToolResult::error("Error (session_busy): The session's shell is busy.")
    );

    let result = call(
        &mut tools,
        "exec_visible",
        json!({ "command": "pwd", "cwd": "/etc", "session_id": "Pane 7" }),
    );
    assert!(result.is_error);
    assert!(
        result
            .text
            .starts_with("Invalid arguments for exec_visible"),
        "{}",
        result.text
    );
}
