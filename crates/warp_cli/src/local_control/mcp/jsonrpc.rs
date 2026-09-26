//! The JSON-RPC 2.0 side of the MCP server: one message per line on stdin and stdout.
//!
//! Requests are answered one at a time, in order. The server never sends requests of its own, so
//! responses from the client are ignored.
use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

/// Protocol versions this server speaks, newest first.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const SERVER_NAME: &str = "warp-agent-bridge";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Outcome of a tool call. A failed tool is a result the model reads, not a protocol error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// What the server exposes to the client.
pub(super) trait McpHandler {
    /// Guidance the client may add to the model's context.
    fn instructions(&self) -> &str;

    /// Called with `clientInfo.name` from `initialize`.
    fn set_client_name(&mut self, name: &str);

    /// The `tools` array of `tools/list`.
    fn tools(&self) -> Value;

    /// `None` when there is no tool with that name.
    fn call_tool(&mut self, name: &str, arguments: Value) -> Option<ToolResult>;
}

/// Answers the messages read from `input` until it ends.
pub(super) fn serve(
    mut input: impl BufRead,
    mut output: impl Write,
    handler: &mut impl McpHandler,
) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if let Some(response) = handle_message(&line, handler) {
            let mut encoded = serde_json::to_vec(&response).map_err(io::Error::other)?;
            encoded.push(b'\n');
            output.write_all(&encoded)?;
            output.flush()?;
        }
    }
}

/// The response to one line, or `None` when the line needs none (a notification or a response).
pub(super) fn handle_message(line: &[u8], handler: &mut impl McpHandler) -> Option<Value> {
    let message: Value = match serde_json::from_slice(line) {
        Ok(message) => message,
        Err(err) => {
            return Some(error_response(
                Value::Null,
                PARSE_ERROR,
                &format!("parse error: {err}"),
            ));
        }
    };
    let Some(object) = message.as_object() else {
        return Some(error_response(
            Value::Null,
            INVALID_REQUEST,
            "expected a JSON-RPC message object",
        ));
    };
    let id = object.get("id").cloned();
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return match id {
            // A response to a request this server never sent.
            Some(_) if object.contains_key("result") || object.contains_key("error") => None,
            id => Some(error_response(
                id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "missing method",
            )),
        };
    };
    let id = id?;
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let outcome = match method {
        "initialize" => Ok(initialize(&params, handler)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": handler.tools() })),
        "tools/call" => call_tool(params, handler),
        method => Err((METHOD_NOT_FOUND, format!("method not found: {method}"))),
    };
    Some(match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn initialize(params: &Value, handler: &mut impl McpHandler) -> Value {
    if let Some(name) = params
        .pointer("/clientInfo/name")
        .and_then(Value::as_str)
    {
        handler.set_client_name(name);
    }
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "version": crate::version_string() },
        "instructions": handler.instructions(),
    })
}

fn call_tool(params: Value, handler: &mut impl McpHandler) -> Result<Value, (i64, String)> {
    let Value::Object(mut params) = params else {
        return Err((INVALID_PARAMS, "tools/call needs params".to_owned()));
    };
    let Some(Value::String(name)) = params.remove("name") else {
        return Err((INVALID_PARAMS, "tools/call needs a tool name".to_owned()));
    };
    let arguments = match params.remove("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(arguments @ Value::Object(_)) => arguments,
        Some(_) => {
            return Err((
                INVALID_PARAMS,
                "tool arguments must be an object".to_owned(),
            ));
        }
    };
    let result = handler
        .call_tool(&name, arguments)
        .ok_or_else(|| (INVALID_PARAMS, format!("unknown tool: {name}")))?;
    Ok(json!({
        "content": [{ "type": "text", "text": result.text }],
        "isError": result.is_error,
    }))
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
#[path = "jsonrpc_tests.rs"]
mod tests;
