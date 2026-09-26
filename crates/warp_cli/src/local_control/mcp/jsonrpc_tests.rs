use std::io::Cursor;

use serde_json::{Value, json};

use super::*;

#[derive(Default)]
struct FakeHandler {
    client_name: Option<String>,
    calls: Vec<(String, Value)>,
}

impl McpHandler for FakeHandler {
    fn instructions(&self) -> &str {
        "use the tools"
    }

    fn set_client_name(&mut self, name: &str) {
        self.client_name = Some(name.to_owned());
    }

    fn tools(&self) -> Value {
        json!([{ "name": "echo" }])
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Option<ToolResult> {
        self.calls.push((name.to_owned(), arguments.clone()));
        match name {
            "echo" => Some(ToolResult::ok(arguments.to_string())),
            "fail" => Some(ToolResult::error("it failed")),
            _ => None,
        }
    }
}

fn handle(line: &str, handler: &mut FakeHandler) -> Option<Value> {
    handle_message(line.as_bytes(), handler)
}

fn error_code(response: &Value) -> Option<i64> {
    response.pointer("/error/code").and_then(Value::as_i64)
}

#[test]
fn initialize_keeps_a_supported_version_and_records_the_client() {
    let mut handler = FakeHandler::default();
    let response = handle(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"claude-code","version":"2.0"}}}"#,
        &mut handler,
    )
    .expect("initialize is answered");

    assert_eq!(response["id"], json!(1));
    assert_eq!(response["result"]["protocolVersion"], json!("2025-03-26"));
    assert_eq!(
        response["result"]["capabilities"],
        json!({ "tools": { "listChanged": false } })
    );
    assert_eq!(
        response["result"]["serverInfo"]["name"],
        json!("warp-agent-bridge")
    );
    assert_eq!(response["result"]["instructions"], json!("use the tools"));
    assert_eq!(handler.client_name.as_deref(), Some("claude-code"));
}

#[test]
fn initialize_answers_an_unknown_version_with_the_newest_one() {
    let mut handler = FakeHandler::default();
    let response = handle(
        r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        &mut handler,
    )
    .expect("initialize is answered");

    assert_eq!(response["id"], json!("a"));
    assert_eq!(response["result"]["protocolVersion"], json!("2025-06-18"));
    assert_eq!(handler.client_name, None);
}

#[test]
fn ping_and_tools_list_are_answered() {
    let mut handler = FakeHandler::default();

    let ping = handle(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#, &mut handler);
    assert_eq!(ping, Some(json!({ "jsonrpc": "2.0", "id": 2, "result": {} })));

    let list = handle(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#,
        &mut handler,
    )
    .expect("tools/list is answered");
    assert_eq!(list["result"]["tools"], json!([{ "name": "echo" }]));
}

#[test]
fn tools_call_wraps_the_result_as_text_content() {
    let mut handler = FakeHandler::default();
    let response = handle(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"x":1}}}"#,
        &mut handler,
    )
    .expect("tools/call is answered");

    assert_eq!(
        response["result"],
        json!({ "content": [{ "type": "text", "text": "{\"x\":1}" }], "isError": false })
    );
}

#[test]
fn a_failed_tool_is_a_result_not_a_protocol_error() {
    let mut handler = FakeHandler::default();
    let response = handle(
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"fail"}}"#,
        &mut handler,
    )
    .expect("tools/call is answered");

    assert_eq!(response["result"]["isError"], json!(true));
    assert_eq!(response["result"]["content"][0]["text"], json!("it failed"));
    assert_eq!(handler.calls, vec![("fail".to_owned(), json!({}))]);
}

#[test]
fn bad_tools_call_params_are_invalid_params() {
    let mut handler = FakeHandler::default();
    for line in [
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call"}"#,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"echo","arguments":[1]}}"#,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope"}}"#,
    ] {
        let response = handle(line, &mut handler).expect("an error is answered");
        assert_eq!(error_code(&response), Some(-32602), "{line}");
        assert_eq!(response["id"], json!(6));
    }
}

#[test]
fn unknown_methods_are_method_not_found() {
    let mut handler = FakeHandler::default();
    let response = handle(
        r#"{"jsonrpc":"2.0","id":7,"method":"resources/list"}"#,
        &mut handler,
    )
    .expect("an error is answered");
    assert_eq!(error_code(&response), Some(-32601));
}

#[test]
fn broken_json_is_a_parse_error_with_a_null_id() {
    let mut handler = FakeHandler::default();
    let response = handle(r#"{"jsonrpc":"2.0","id":8,"#, &mut handler).expect("answered");
    assert_eq!(error_code(&response), Some(-32700));
    assert_eq!(response["id"], Value::Null);

    let response = handle("[1, 2]", &mut handler).expect("answered");
    assert_eq!(error_code(&response), Some(-32600));
}

#[test]
fn notifications_and_responses_get_no_answer() {
    let mut handler = FakeHandler::default();
    assert_eq!(
        handle(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &mut handler
        ),
        None
    );
    assert_eq!(
        handle(
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"echo"}}"#,
            &mut handler
        ),
        None
    );
    assert_eq!(
        handle(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#, &mut handler),
        None
    );
    assert!(handler.calls.is_empty());
}

#[test]
fn serve_writes_one_line_per_answer_and_stops_at_the_end_of_input() {
    let mut handler = FakeHandler::default();
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        "\n\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{"s":"a\nb"}}}"#,
    );
    let mut output = Vec::new();

    serve(Cursor::new(input), &mut output, &mut handler).expect("serve succeeds");

    let output = String::from_utf8(output).expect("utf-8 output");
    let lines: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["id"], json!(1));
    assert_eq!(lines[1]["id"], json!(2));
    assert!(output.ends_with('\n'));
}

#[test]
fn serve_answers_invalid_utf8_with_a_parse_error() {
    let mut handler = FakeHandler::default();
    let mut output = Vec::new();

    serve(Cursor::new(b"\xff\xfe\n".to_vec()), &mut output, &mut handler).expect("serve succeeds");

    let response: Value = serde_json::from_slice(&output).expect("JSON answer");
    assert_eq!(error_code(&response), Some(-32700));
}
