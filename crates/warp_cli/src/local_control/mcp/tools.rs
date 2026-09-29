//! The MCP tools and what they ask of Warp.
use std::collections::HashMap;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use local_control::protocol::{
    APPROVAL_TIMEOUT_SECS, ActionKind, AgentToken, ControlError, RemoteExecParams,
    RemoteExecResult, RemoteExecVisibleParams, RemoteExecVisibleResult, RemoteFileReadParams,
    RemoteFileReadResult, RemoteFileWriteParams, RemoteFileWriteResult, RemoteHostListParams,
    RemoteHostListResult, RemoteOutputRecentParams, RemoteOutputRecentResult,
    RemoteSessionListResult, RemoteSessionRef, WriteExpectation,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::edit::{EditError, apply_edit};
use super::format::{
    edit_snippet, place, render_exec, render_exec_visible, render_hosts, render_read,
    render_sessions,
};
use super::jsonrpc::{McpHandler, ToolResult};
use super::redact::Redactor;
use crate::local_control::remote::{
    APPROVAL_CLIENT_MARGIN, EXEC_CLIENT_MARGIN, EXEC_DEFAULT_TIMEOUT_SECS, FILE_CLIENT_TIMEOUT,
    HOSTS_DEFAULT_LIMIT, HOSTS_MAX_LIMIT, MAX_WRITE_BYTES, RECENT_DEFAULT_COUNT, RECENT_MAX_COUNT,
    SESSIONS_CLIENT_TIMEOUT, decode, render_recent,
};

/// Audit name when the client does not say who it is.
const UNKNOWN_AGENT: &str = "mcp-unknown";

/// The app accepts agent names of at most this many bytes.
const MAX_AGENT_NAME_BYTES: usize = 64;

const DEFAULT_READ_LIMIT: usize = 2000;

/// A run of this many `*` in text the model sends back is taken for a hidden secret.
const HIDDEN_RUN_MIN_LEN: usize = 4;

pub(super) const INSTRUCTIONS: &str = "\
These tools act on a remote server through a terminal session the user opened in Warp (usually \
`ssh` then `sudo -i`, so commands often run as root). They are not your local shell: use your own \
Bash/Read/Edit tools for this machine and these tools for the server.
- Call list_sessions first and name the user@host you are about to change before any change.
- list_hosts shows the servers the user keeps in Warp (tags, user@host, open sessions, the local \
mirror of their files); it does not open sessions. Read a mirror with your own local tools; \
changing a server's files still goes through read_file and edit_file.
- When the user refers to something they just ran or an error they just saw in that terminal, \
call recent_output to read it instead of running the command again.
- Prefer read_file + edit_file for config files; edit_file only needs the lines you change.
- Check syntax before reloading a service (nginx -t, sshd -t, visudo -c, apachectl configtest, \
systemd-analyze verify) and verify the result afterwards.
- Do not restart sshd, networking or the firewall unless the user explicitly confirmed it: a \
mistake can lock everyone out of the server.
- Commands get no terminal and no stdin: pass -y, --no-pager and similar flags; interactive \
programs fail.
- exec times out after 120 s by default (timeout_secs, at most 600). Long output is cut in the \
middle: narrow it with grep, head or tail.
- exec runs out of sight. Use exec_visible only when the user wants to watch a command, when a \
`cd` or `export` must stay in effect, or when a command may ask a question the user will answer \
in the terminal. It runs in the user's own shell and history; never send exit, logout, exec, su \
or sudo -i with it (leaving the shell ends the agent's access).
- Text that looks like a secret is shown as ****. Such text cannot be matched by edit_file, and \
write_file refuses to overwrite files that contain it.
- Warp may ask the user to approve a write; the call then waits up to 5 minutes. If the result \
says \"Denied by Warp's agent policy\", do not retry the same command or rephrase it to get \
around the rule; tell the user why it was denied.
- Keep commands short and single-purpose so the user can review them.";

/// How the tools reach Warp; a fake in tests.
pub(super) trait ControlTransport {
    /// Sends `action` to Warp, addressed to the session with this id when there is one.
    fn call(
        &mut self,
        action: ActionKind,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, ControlError>;

    /// Sends `agent.pair`'s handshake with `token` under `name`. On success or failure, a
    /// conforming implementation remembers `token` for every later `call` (mục 3.11 of the O2
    /// agent-ops policy plan) — Warp simply treats an unrecognized token as an unpaired client.
    fn pair(
        &mut self,
        name: &str,
        token: &AgentToken,
        timeout: Duration,
    ) -> Result<Value, ControlError>;
}

/// A failed tool call: the message the model reads.
type ToolError = String;

pub(super) struct Tools<T> {
    transport: T,
    redactor: Redactor,
    agent: String,
    /// SHA-256 of each file as the model last saw it, by session id and absolute path. Files are
    /// only overwritten or edited in that state.
    seen_versions: HashMap<(String, String), String>,
    /// `--no-pair`: never attempt pairing, so `agent_token` never leaves this process (mục 3.11 of
    /// the O2 agent-ops policy plan).
    no_pair: bool,
    /// Whether pairing has already been attempted this process — tried at most once, on the first
    /// tool call, regardless of whether it succeeds.
    pairing_attempted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListSessionsArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListHostsArgs {
    query: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecArgs {
    command: String,
    cwd: Option<String>,
    timeout_secs: Option<u32>,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecVisibleArgs {
    command: String,
    timeout_secs: Option<u32>,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFileArgs {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteFileArgs {
    path: String,
    content: String,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecentOutputArgs {
    count: Option<u32>,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
    session_id: Option<String>,
}

/// A file read without being shown to the model.
struct RawFile {
    session: RemoteSessionRef,
    path: String,
    sha256: String,
    bytes: Vec<u8>,
}

impl<T: ControlTransport> Tools<T> {
    pub fn new(transport: T, redactor: Redactor, no_pair: bool) -> Self {
        Self {
            transport,
            redactor,
            agent: UNKNOWN_AGENT.to_owned(),
            seen_versions: HashMap::new(),
            no_pair,
            pairing_attempted: false,
        }
    }

    /// Pairs with Warp once per process, on the first tool call, using a token persisted under
    /// this client's name (mục 3.11 of the O2 agent-ops policy plan). A failure or denial is not
    /// retried in this process — it just prints why, and leaves the token attached to every later
    /// call anyway, since Warp treats an unrecognized token the same as no token at all.
    fn maybe_pair(&mut self) {
        if self.pairing_attempted || self.no_pair {
            return;
        }
        self.pairing_attempted = true;
        let token = match crate::local_control::pairing::token_for(&self.agent) {
            Ok(token) => token,
            Err(err) => {
                eprintln!(
                    "warpctrl: could not prepare a pairing token for '{}': {err}",
                    self.agent
                );
                return;
            }
        };
        let timeout = Duration::from_secs(APPROVAL_TIMEOUT_SECS) + Duration::from_secs(30);
        if let Err(err) = self.transport.pair(&self.agent, &token, timeout) {
            eprintln!(
                "warpctrl: pairing with Warp did not complete: {}",
                control_error_text(&err)
            );
        }
    }

    fn list_sessions(&mut self, args: Value) -> Result<String, ToolError> {
        let ListSessionsArgs {} = parse_args("list_sessions", args)?;
        let result = self.session_list()?;
        Ok(render_sessions(&result.sessions))
    }

    fn list_hosts(&mut self, args: Value) -> Result<String, ToolError> {
        let args: ListHostsArgs = parse_args("list_hosts", args)?;
        let params = RemoteHostListParams {
            query: args.query,
            limit: Some(
                args.limit
                    .unwrap_or(HOSTS_DEFAULT_LIMIT)
                    .clamp(1, HOSTS_MAX_LIMIT),
            ),
        };
        let result: RemoteHostListResult = self.call(
            ActionKind::RemoteHostList,
            params,
            None,
            SESSIONS_CLIENT_TIMEOUT,
        )?;
        Ok(render_hosts(&result))
    }

    fn exec(&mut self, args: Value) -> Result<String, ToolError> {
        let args: ExecArgs = parse_args("exec", args)?;
        let session_id = self.resolve_session(args.session_id)?;
        let timeout_secs = args.timeout_secs.unwrap_or(EXEC_DEFAULT_TIMEOUT_SECS);
        let params = RemoteExecParams {
            command: args.command,
            cwd: args.cwd,
            timeout_secs: Some(timeout_secs),
            agent: Some(self.agent.clone()),
        };
        let wait =
            Duration::from_secs(timeout_secs.into()) + EXEC_CLIENT_MARGIN + APPROVAL_CLIENT_MARGIN;
        let result: RemoteExecResult =
            self.call(ActionKind::RemoteExec, params, Some(&session_id), wait)?;
        Ok(render_exec(&result))
    }

    fn exec_visible(&mut self, args: Value) -> Result<String, ToolError> {
        let args: ExecVisibleArgs = parse_args("exec_visible", args)?;
        let session_id = self.resolve_session(args.session_id)?;
        let timeout_secs = args.timeout_secs.unwrap_or(EXEC_DEFAULT_TIMEOUT_SECS);
        let params = RemoteExecVisibleParams {
            command: args.command,
            timeout_secs: Some(timeout_secs),
            agent: Some(self.agent.clone()),
        };
        let wait =
            Duration::from_secs(timeout_secs.into()) + EXEC_CLIENT_MARGIN + APPROVAL_CLIENT_MARGIN;
        let result: RemoteExecVisibleResult = self.call(
            ActionKind::RemoteExecVisible,
            params,
            Some(&session_id),
            wait,
        )?;
        Ok(render_exec_visible(&result))
    }

    fn read_file(&mut self, args: Value) -> Result<String, ToolError> {
        let args: ReadFileArgs = parse_args("read_file", args)?;
        let session_id = self.resolve_session(args.session_id)?;
        let file = self
            .read_raw(&session_id, &args.path)?
            .ok_or_else(|| format!("{} does not exist.", args.path))?;
        let content = String::from_utf8(file.bytes).map_err(|_| {
            format!(
                "{} is a binary file; inspect it with exec (file, xxd | head, strings).",
                place(&file.session, &file.path)
            )
        })?;
        self.remember(&file.session, &file.path, file.sha256);
        let visible = self.redactor.to_model_text(&content);
        let first_line = args.offset.map_or(1, to_usize);
        let limit = args.limit.map_or(DEFAULT_READ_LIMIT, to_usize).max(1);
        Ok(render_read(
            &file.session,
            &file.path,
            &visible,
            first_line,
            limit,
        ))
    }

    fn write_file(&mut self, args: Value) -> Result<String, ToolError> {
        let args: WriteFileArgs = parse_args("write_file", args)?;
        check_write_size(args.content.len())?;
        let session_id = self.resolve_session(args.session_id)?;
        let (path, expectation) = match self.read_raw(&session_id, &args.path)? {
            None => (args.path, WriteExpectation::MustNotExist),
            Some(file) => {
                if std::str::from_utf8(&file.bytes).is_err() {
                    return Err(format!(
                        "{} is a binary file; write_file only replaces text files.",
                        place(&file.session, &file.path)
                    ));
                }
                self.check_seen(&file)?;
                if self
                    .redactor
                    .contains_secrets(&String::from_utf8_lossy(&file.bytes))
                {
                    return Err(format!(
                        "{} contains secrets that were hidden from you, so it cannot be \
                         rewritten whole. Change it with edit_file instead.",
                        place(&file.session, &file.path)
                    ));
                }
                (
                    file.path,
                    WriteExpectation::MustMatch {
                        sha256: file.sha256,
                    },
                )
            }
        };
        let result = self.write_raw(&session_id, path, args.content.as_bytes(), expectation)?;
        let verb = if result.created { "Created" } else { "Wrote" };
        Ok(format!(
            "{verb} {} ({} bytes).{}",
            place(&result.session, &result.path),
            result.bytes,
            backup_note(&result)
        ))
    }

    fn edit_file(&mut self, args: Value) -> Result<String, ToolError> {
        let args: EditFileArgs = parse_args("edit_file", args)?;
        let session_id = self.resolve_session(args.session_id)?;
        let file = self
            .read_raw(&session_id, &args.path)?
            .ok_or_else(|| format!("{} does not exist; create it with write_file.", args.path))?;
        self.check_seen(&file)?;
        let where_ = place(&file.session, &file.path);
        let content = String::from_utf8(file.bytes)
            .map_err(|_| format!("{where_} is a binary file and cannot be edited."))?;
        let visible = self.redactor.to_model_text(&content);
        if copies_hidden_text(&content, &visible, &args.new_string) {
            return Err(
                "new_string contains **** that stands for a hidden secret; writing it would \
                 replace the secret with stars. Leave the lines with hidden secrets out of the \
                 edit."
                    .to_owned(),
            );
        }
        let edit = apply_edit(
            &content,
            &args.old_string,
            &args.new_string,
            args.replace_all,
        )
        .map_err(|err| edit_error_text(err, &where_, &visible, &args.old_string))?;
        check_write_size(edit.content.len())?;
        let result = self.write_raw(
            &session_id,
            file.path,
            edit.content.as_bytes(),
            WriteExpectation::MustMatch {
                sha256: file.sha256,
            },
        )?;
        let new_visible = self.redactor.to_model_text(&edit.content);
        let occurrences = match edit.replacements {
            1 => "1 occurrence".to_owned(),
            count => format!("{count} occurrences"),
        };
        Ok(format!(
            "Edited {}: replaced {occurrences}.{}\n{}",
            place(&result.session, &result.path),
            backup_note(&result),
            edit_snippet(&new_visible, edit.first_change, &args.new_string)
        ))
    }

    fn recent_output(&mut self, args: Value) -> Result<String, ToolError> {
        let args: RecentOutputArgs = parse_args("recent_output", args)?;
        let session_id = self.resolve_session(args.session_id)?;
        let params = RemoteOutputRecentParams {
            count: Some(
                args.count
                    .unwrap_or(RECENT_DEFAULT_COUNT)
                    .clamp(1, RECENT_MAX_COUNT),
            ),
            agent: Some(self.agent.clone()),
        };
        let result: RemoteOutputRecentResult = self.call(
            ActionKind::RemoteOutputRecent,
            params,
            Some(&session_id),
            SESSIONS_CLIENT_TIMEOUT,
        )?;
        Ok(render_recent(&result))
    }

    /// The session a tool call acts on: the one asked for, or else the only attached one.
    fn resolve_session(&mut self, requested: Option<String>) -> Result<String, ToolError> {
        if let Some(session_id) = requested {
            return Ok(session_id);
        }
        let sessions = self.session_list()?.sessions;
        let attached: Vec<_> = sessions
            .iter()
            .filter(|session| session.attached.is_some())
            .collect();
        match attached.as_slice() {
            [only] => Ok(only.session_id.clone()),
            [] => Err(format!(
                "No session is attached. Ask the user to run 'Agent Bridge: Allow agents to \
                 control this session' from the Warp command palette in the pane of the \
                 server.\n{}",
                render_sessions(&sessions)
            )),
            several => Err(format!(
                "{} sessions are attached; pass session_id to pick one.\n{}",
                several.len(),
                render_sessions(&sessions)
            )),
        }
    }

    fn session_list(&mut self) -> Result<RemoteSessionListResult, ToolError> {
        self.call(
            ActionKind::RemoteSessionList,
            json!({}),
            None,
            SESSIONS_CLIENT_TIMEOUT,
        )
    }

    /// `None` when the file does not exist.
    fn read_raw(&mut self, session_id: &str, path: &str) -> Result<Option<RawFile>, ToolError> {
        let params = RemoteFileReadParams {
            path: path.to_owned(),
            agent: Some(self.agent.clone()),
        };
        let result = self.call(
            ActionKind::RemoteFileRead,
            params,
            Some(session_id),
            FILE_CLIENT_TIMEOUT,
        )?;
        match result {
            RemoteFileReadResult::NotFound { .. } => Ok(None),
            RemoteFileReadResult::Ok {
                session,
                path,
                sha256,
                content_base64,
                ..
            } => {
                let bytes = BASE64
                    .decode(content_base64)
                    .map_err(|err| format!("Warp sent file content that is not base64: {err}"))?;
                Ok(Some(RawFile {
                    session,
                    path,
                    sha256,
                    bytes,
                }))
            }
        }
    }

    fn write_raw(
        &mut self,
        session_id: &str,
        path: String,
        content: &[u8],
        expectation: WriteExpectation,
    ) -> Result<RemoteFileWriteResult, ToolError> {
        let params = RemoteFileWriteParams {
            path,
            content_base64: BASE64.encode(content),
            expectation,
            agent: Some(self.agent.clone()),
        };
        let result: RemoteFileWriteResult = self.call(
            ActionKind::RemoteFileWrite,
            params,
            Some(session_id),
            FILE_CLIENT_TIMEOUT + APPROVAL_CLIENT_MARGIN,
        )?;
        if result.sha256 != sha256_hex(content) {
            return Err(format!(
                "{} was written but its checksum on the server does not match what was sent; \
                 read it again to check it.",
                place(&result.session, &result.path)
            ));
        }
        self.remember(&result.session, &result.path, result.sha256.clone());
        Ok(result)
    }

    /// Only a file in the state the model last saw may be changed.
    fn check_seen(&self, file: &RawFile) -> Result<(), ToolError> {
        let key = (file.session.session_id.clone(), file.path.clone());
        match self.seen_versions.get(&key) {
            None => Err(format!(
                "Read {} with read_file before changing it.",
                place(&file.session, &file.path)
            )),
            Some(seen) if *seen != file.sha256 => Err(format!(
                "{} changed on the server since you read it. Read it again with read_file.",
                place(&file.session, &file.path)
            )),
            Some(_) => Ok(()),
        }
    }

    fn remember(&mut self, session: &RemoteSessionRef, path: &str, sha256: String) {
        self.seen_versions
            .insert((session.session_id.clone(), path.to_owned()), sha256);
    }

    fn call<R: DeserializeOwned>(
        &mut self,
        action: ActionKind,
        params: impl serde::Serialize,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<R, ToolError> {
        let params = serde_json::to_value(params)
            .map_err(|err| format!("could not encode the request: {err}"))?;
        let data = self
            .transport
            .call(action, params, session, timeout)
            .map_err(|err| control_error_text(&err))?;
        decode(data, "result").map_err(|err| control_error_text(&err))
    }
}

impl<T: ControlTransport> McpHandler for Tools<T> {
    fn instructions(&self) -> &str {
        INSTRUCTIONS
    }

    fn set_client_name(&mut self, name: &str) {
        self.agent = agent_name(name);
    }

    fn tools(&self) -> Value {
        tool_definitions()
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Option<ToolResult> {
        self.maybe_pair();
        let outcome = match name {
            "list_sessions" => self.list_sessions(arguments),
            "list_hosts" => self.list_hosts(arguments),
            "exec" => self.exec(arguments),
            "exec_visible" => self.exec_visible(arguments),
            "read_file" => self.read_file(arguments),
            "write_file" => self.write_file(arguments),
            "edit_file" => self.edit_file(arguments),
            "recent_output" => self.recent_output(arguments),
            _ => return None,
        };
        Some(match outcome {
            Ok(text) => ToolResult::ok(self.redactor.to_model_text(&text)),
            Err(text) => ToolResult::error(self.redactor.to_model_text(&text)),
        })
    }
}

fn parse_args<A: DeserializeOwned>(tool: &str, arguments: Value) -> Result<A, ToolError> {
    serde_json::from_value(arguments).map_err(|err| format!("Invalid arguments for {tool}: {err}"))
}

fn control_error_text(err: &ControlError) -> String {
    match &err.details {
        Some(details) => format!("Error ({}): {}\n{details}", err.code, err.message),
        None => format!("Error ({}): {}", err.code, err.message),
    }
}

fn check_write_size(len: usize) -> Result<(), ToolError> {
    if len as u64 > MAX_WRITE_BYTES {
        return Err(format!(
            "The new content is {len} bytes; at most {MAX_WRITE_BYTES} can be written."
        ));
    }
    Ok(())
}

fn backup_note(result: &RemoteFileWriteResult) -> String {
    match &result.backup_path {
        Some(backup) => format!(" The previous content was saved in {backup} on the server."),
        None => String::new(),
    }
}

fn edit_error_text(err: EditError, place: &str, visible: &str, old: &str) -> String {
    match err {
        EditError::EmptyOldString => "old_string must not be empty.".to_owned(),
        EditError::Unchanged => "old_string and new_string are the same.".to_owned(),
        EditError::NotFound if visible.contains(old) => {
            "old_string includes text hidden as ****, which cannot be matched. Pick an \
             old_string that leaves the hidden part out."
                .to_owned()
        }
        EditError::NotFound => format!(
            "old_string was not found in {place}. Read the file again and copy the text exactly, \
             without the line numbers."
        ),
        EditError::Ambiguous { count } => format!(
            "old_string occurs {count} times in {place}. Include more surrounding lines to make it \
             unique, or set replace_all."
        ),
    }
}

/// Whether `new` repeats a run of `*` that the model only saw because a secret was hidden.
fn copies_hidden_text(content: &str, visible: &str, new: &str) -> bool {
    if content == visible {
        return false;
    }
    new.split(|character| character != '*')
        .filter(|run| run.len() >= HIDDEN_RUN_MIN_LEN)
        .any(|run| visible.contains(run) && !content.contains(run))
}

/// `name` as the app accepts it: at most 64 bytes of `[A-Za-z0-9._-]`.
fn agent_name(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|character| match character {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-' => character,
            _ => '-',
        })
        .take(MAX_AGENT_NAME_BYTES)
        .collect();
    if name.is_empty() {
        UNKNOWN_AGENT.to_owned()
    } else {
        name
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn session_id_schema() -> Value {
    json!({
        "type": "string",
        "description": "Session to use, as shown by list_sessions. May be left out when exactly \
                        one session is attached."
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_sessions",
            "title": "List Warp sessions",
            "description": "List the terminal sessions open in Warp: user@host, current \
                            directory and whether the user allowed agents to use each one.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "list_hosts",
            "title": "List the user's servers",
            "description": "List the servers in the user's Warp server directory: alias, tags, \
                            user@host:port, how to become root, the sessions open on each, and \
                            the local folder that mirrors its synced files with the paths synced. \
                            Only describes; it does not connect and holds no passwords or keys.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Words that match aliases and tags loosely; tag:prod \
                                        keeps only servers with that tag."
                    },
                    "limit": {
                        "type": "integer", "minimum": 1, "maximum": 100,
                        "description": "How many servers to show (default 20)."
                    }
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "exec",
            "title": "Run a command on the server",
            "description": "Run a shell command on the remote server in an attached Warp \
                            session, with that session's user (often root). No terminal, no \
                            stdin. Returns the exit code, stdout and stderr.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run." },
                    "cwd": {
                        "type": "string",
                        "description": "Absolute directory to run in; defaults to the \
                                        session's current directory."
                    },
                    "timeout_secs": {
                        "type": "integer", "minimum": 1, "maximum": 600,
                        "description": "Seconds before the command is stopped (default 120)."
                    },
                    "session_id": session_id_schema()
                },
                "required": ["command"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": true }
        },
        {
            "name": "exec_visible",
            "title": "Run a command in the user's terminal",
            "description": "Type a shell command into the user's own shell in an attached Warp \
                            session, where it runs as a block the user watches. Unlike exec, \
                            cd and export stay in effect, the command enters the shell history, \
                            the user can answer prompts in the terminal, and the output is what \
                            the terminal shows (stdout and stderr together). A command still \
                            running after timeout_secs keeps running and is reported as still \
                            running. Prefer exec unless the user should see the command.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to type, on a single line (join commands \
                                        with ';' or '&&')."
                    },
                    "timeout_secs": {
                        "type": "integer", "minimum": 1, "maximum": 600,
                        "description": "Seconds to wait for the command before answering with \
                                        its output so far (default 120)."
                    },
                    "session_id": session_id_schema()
                },
                "required": ["command"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": true }
        },
        {
            "name": "read_file",
            "title": "Read a file on the server",
            "description": "Read a text file on the remote server, with line numbers (cat -n). \
                            Needed before write_file or edit_file change an existing file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path, or relative to the session's directory."
                    },
                    "offset": {
                        "type": "integer", "minimum": 1,
                        "description": "First line to show (from 1)."
                    },
                    "limit": {
                        "type": "integer", "minimum": 1,
                        "description": "Number of lines to show (default 2000)."
                    },
                    "session_id": session_id_schema()
                },
                "required": ["path"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "recent_output",
            "title": "Show the user's recent commands",
            "description": "Show the latest commands the user ran in an attached Warp session, \
                            with exit code, directory and output (long output is cut in the \
                            middle). Reads what the terminal already shows; nothing runs on the \
                            server.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "count": {
                        "type": "integer", "minimum": 1, "maximum": 10,
                        "description": "How many of the latest commands to show (default 3)."
                    },
                    "session_id": session_id_schema()
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "write_file",
            "title": "Write a file on the server",
            "description": "Create a file on the remote server, or replace one you read with \
                            read_file and that has not changed since. The previous content is \
                            backed up on the server. Prefer edit_file for changes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path on the server." },
                    "content": { "type": "string", "description": "The whole new content." },
                    "session_id": session_id_schema()
                },
                "required": ["path", "content"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false }
        },
        {
            "name": "edit_file",
            "title": "Edit a file on the server",
            "description": "Replace old_string with new_string in a file on the remote server \
                            that you read with read_file. old_string must match exactly once \
                            unless replace_all is set. The previous content is backed up on \
                            the server.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path on the server." },
                    "old_string": {
                        "type": "string",
                        "description": "Exact text to replace, without line numbers."
                    },
                    "new_string": { "type": "string", "description": "Replacement text." },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace every occurrence (default false)."
                    },
                    "session_id": session_id_schema()
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false }
        }
    ])
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
