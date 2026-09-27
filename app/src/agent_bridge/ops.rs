//! The operations behind the `remote.*` actions. Each one validates its parameters before
//! anything runs on the server, runs its scripts through a [`CommandRunner`], and records the
//! request in the audit log whether it succeeded or not.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ::local_control::ActionKind;
use ::local_control::protocol::{
    RemoteExecParams, RemoteExecResult, RemoteFileReadParams, RemoteFileReadResult,
    RemoteFileWriteParams, RemoteFileWriteResult, RemoteOutputRecentResult, RemoteSessionRef,
    RemoteStream, WriteExpectation,
};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Utc;
use instant::Instant;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;
use warp_completer::completer::CommandExitStatus;
use warpui::r#async::FutureExt as _;

use super::audit::{self, AuditOutcome, AuditRecord};
use super::error::AgentBridgeError;
use super::path::{normalize_path, validate_cwd};
use super::recent::{CapturedBlock, command_block};
use super::script::{
    ExecOutput, ReadOutcome, Stream, WriteOutcome, backup_name, exec_script, new_nonce,
    parse_exec_output, parse_read_output, parse_write_output, read_script, sha256_hex,
    write_commit_script,
};
use super::{
    EXEC_DEFAULT_TIMEOUT_SECS, EXEC_MAX_TIMEOUT_SECS, EXEC_TIMEOUT_GRACE, MAX_COMMAND_BYTES,
    READ_MAX_FILE_BYTES, WRITE_MAX_BYTES,
};
use crate::terminal::model::session::{ExecuteCommandOptions, Session, SessionType};
use crate::terminal::shell::ShellType;
use crate::warp_sync::COMMAND_TIMEOUT;
use crate::warp_sync::remote_script::{
    PAYLOAD_FILE_NAME, RemoteTmpDir, cleanup_command, posix_quote, remote_failure_message,
    upload_begin_command, upload_chunk_commands, validate_tmp_dir, wrap_for_any_shell,
};

const MAX_AGENT_NAME_BYTES: usize = 64;
const SHA256_HEX_LEN: usize = 64;

/// Length of `WRITE_MAX_BYTES` of content in base64, with room for padding and whitespace.
const WRITE_MAX_BASE64_LEN: usize = WRITE_MAX_BYTES / 3 * 4 + 8;

/// What a command printed and whether the shell reported it as successful.
#[derive(Debug, Default)]
pub(crate) struct RawOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub success: bool,
}

/// A shell on the remote host that commands can be run in.
#[async_trait]
pub(crate) trait CommandRunner: Send + Sync {
    /// Runs `command` in the shell and waits at most `timeout` for it.
    async fn run(&self, command: &str, timeout: Duration) -> Result<RawOutput, AgentBridgeError>;
}

/// Runs commands through the command executor of a Warpified remote session, so that they run
/// with the privileges of the shell the user is looking at.
pub(crate) struct SessionRunner {
    session: Arc<Session>,
}

impl SessionRunner {
    pub(crate) fn new(session: Arc<Session>) -> Result<Self, AgentBridgeError> {
        ensure_supported(&session)?;
        Ok(Self { session })
    }
}

/// Checks that agents can use `session` at all: it has to be a Warpified remote session whose
/// shell the scripts run in.
pub(crate) fn ensure_supported(session: &Session) -> Result<(), AgentBridgeError> {
    match session.session_type() {
        SessionType::WarpifiedRemote { .. } => {}
        SessionType::Local => return Err(AgentBridgeError::NotRemoteSession),
    }
    match session.shell().shell_type() {
        ShellType::PowerShell => Err(AgentBridgeError::UnsupportedShell),
        ShellType::Zsh | ShellType::Bash | ShellType::Fish => Ok(()),
    }
}

#[async_trait]
impl CommandRunner for SessionRunner {
    async fn run(&self, command: &str, timeout: Duration) -> Result<RawOutput, AgentBridgeError> {
        let output = self
            .session
            .execute_command(command, None, None, ExecuteCommandOptions::default())
            .with_timeout(timeout)
            .await
            .map_err(|_| AgentBridgeError::Timeout {
                secs: timeout.as_secs(),
            })?
            .map_err(|err| AgentBridgeError::Executor(format!("{err:#}")))?;
        // The in-band executor answers a command that the PTY controller cancelled, because the
        // user is running something in the foreground, with an empty failure and no exit code.
        // Every script the bridge runs prints something.
        let was_cancelled = output.status == CommandExitStatus::Failure
            && output.exit_code.is_none()
            && output.stdout.is_empty()
            && output.stderr.is_empty();
        if was_cancelled {
            return Err(AgentBridgeError::SessionBusy);
        }
        Ok(RawOutput {
            success: output.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// The session a request acts on and where its audit record goes.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    pub session: RemoteSessionRef,
    /// Current directory of the session: the default for a command and the base of relative
    /// paths.
    pub cwd: Option<String>,
    pub request_id: Uuid,
    /// Where to write the audit record; `None` skips auditing.
    pub audit_dir: Option<PathBuf>,
}

pub(crate) async fn exec(
    runner: &dyn CommandRunner,
    target: &Target,
    params: RemoteExecParams,
) -> Result<Value, AgentBridgeError> {
    let mut audit = Audit::begin(target, ActionKind::RemoteExec, params.agent.as_deref());
    audit.record.command = Some(params.command.clone());
    audit.record.cwd = params.cwd.clone().or_else(|| target.cwd.clone());
    audit.record_start()?;
    let result = run_exec(runner, target, &params).await;
    audit.finish(
        result
            .as_ref()
            .map(|result| Details::exec(result.exit_code))
            .map_err(Clone::clone),
    );
    to_json(result?)
}

pub(crate) async fn read_file(
    runner: &dyn CommandRunner,
    target: &Target,
    params: RemoteFileReadParams,
) -> Result<Value, AgentBridgeError> {
    let mut audit = Audit::begin(target, ActionKind::RemoteFileRead, params.agent.as_deref());
    audit.record.path = Some(params.path.clone());
    audit.record_start()?;
    let result = run_read(runner, target, &params).await;
    audit.finish(
        result
            .as_ref()
            .map(|(_, size)| Details::bytes(*size))
            .map_err(Clone::clone),
    );
    to_json(result?.0)
}

pub(crate) async fn write_file(
    runner: &dyn CommandRunner,
    target: &Target,
    params: RemoteFileWriteParams,
) -> Result<Value, AgentBridgeError> {
    let mut audit = Audit::begin(target, ActionKind::RemoteFileWrite, params.agent.as_deref());
    audit.record.path = Some(params.path.clone());
    audit.record_start()?;
    let result = run_write(runner, target, &params).await;
    audit.finish(
        result
            .as_ref()
            .map(|result| Details::bytes(result.bytes))
            .map_err(Clone::clone),
    );
    to_json(result?)
}

/// Returns blocks that were already copied out of the terminal, audited like a file read.
pub(crate) fn recent_output(
    target: &Target,
    agent: Option<&str>,
    blocks: Vec<CapturedBlock>,
) -> Result<Value, AgentBridgeError> {
    let mut audit = Audit::begin(target, ActionKind::RemoteOutputRecent, agent);
    audit.record_start()?;
    let blocks = blocks.into_iter().map(command_block).collect::<Vec<_>>();
    let bytes = blocks.iter().map(|block| block.output.len() as u64).sum();
    audit.finish(Ok(Details::bytes(bytes)));
    to_json(RemoteOutputRecentResult {
        session: target.session.clone(),
        blocks,
    })
}

fn to_json(result: impl Serialize) -> Result<Value, AgentBridgeError> {
    serde_json::to_value(result)
        .map_err(|err| AgentBridgeError::Io(format!("could not encode the result: {err}")))
}

// ---------------------------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------------------------

pub(crate) fn validate_agent(agent: Option<&str>) -> Result<(), AgentBridgeError> {
    let Some(agent) = agent else {
        return Ok(());
    };
    let is_valid = !agent.is_empty()
        && agent.len() <= MAX_AGENT_NAME_BYTES
        && agent
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if is_valid {
        Ok(())
    } else {
        Err(AgentBridgeError::InvalidParams(format!(
            "agent must be 1-{MAX_AGENT_NAME_BYTES} characters of letters, digits, '.', '_' or '-'"
        )))
    }
}

fn validate_exec(params: &RemoteExecParams) -> Result<(), AgentBridgeError> {
    validate_command(&params.command, params.timeout_secs, params.agent.as_deref())?;
    params.cwd.as_deref().map_or(Ok(()), validate_cwd)
}

/// Checks the parameters `remote.exec` and `remote.exec.visible` share.
pub(crate) fn validate_command(
    command: &str,
    timeout_secs: Option<u32>,
    agent: Option<&str>,
) -> Result<(), AgentBridgeError> {
    validate_agent(agent)?;
    if command.trim().is_empty() {
        return Err(AgentBridgeError::InvalidParams(
            "command is empty".to_owned(),
        ));
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err(AgentBridgeError::InvalidParams(format!(
            "command is longer than {MAX_COMMAND_BYTES} bytes; write a script file instead"
        )));
    }
    if command.contains('\0') {
        return Err(AgentBridgeError::InvalidParams(
            "command contains a NUL byte".to_owned(),
        ));
    }
    if let Some(timeout_secs) = timeout_secs
        && !(1..=EXEC_MAX_TIMEOUT_SECS).contains(&timeout_secs)
    {
        return Err(AgentBridgeError::InvalidParams(format!(
            "timeout_secs must be between 1 and {EXEC_MAX_TIMEOUT_SECS}"
        )));
    }
    Ok(())
}

fn validate_write(params: &RemoteFileWriteParams) -> Result<(), AgentBridgeError> {
    validate_agent(params.agent.as_deref())?;
    if let WriteExpectation::MustMatch { sha256 } = &params.expectation {
        let is_hex = sha256.len() == SHA256_HEX_LEN
            && sha256
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        if !is_hex {
            return Err(AgentBridgeError::InvalidParams(
                "expectation.sha256 must be 64 lowercase hex characters".to_owned(),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// exec
// ---------------------------------------------------------------------------------------------

async fn run_exec(
    runner: &dyn CommandRunner,
    target: &Target,
    params: &RemoteExecParams,
) -> Result<RemoteExecResult, AgentBridgeError> {
    validate_exec(params)?;
    let timeout_secs = params.timeout_secs.unwrap_or(EXEC_DEFAULT_TIMEOUT_SECS);
    // The session's own directory comes from the shell, so an odd one is dropped rather than
    // failing the command.
    let cwd = params
        .cwd
        .clone()
        .or_else(|| target.cwd.clone().filter(|cwd| validate_cwd(cwd).is_ok()));

    let nonce = new_nonce();
    let script = exec_script(&nonce, &params.command, cwd.as_deref(), timeout_secs);
    let wait = Duration::from_secs(timeout_secs.into()) + EXEC_TIMEOUT_GRACE;
    let started = Instant::now();
    let output = runner.run(&wrap_for_any_shell(&script), wait).await?;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    let ExecOutput {
        stdout,
        stderr,
        exit_code,
        timed_out,
    } = parse_exec_output(&nonce, &output.stdout, &output.stderr)?;
    Ok(RemoteExecResult {
        session: target.session.clone(),
        cwd,
        exit_code,
        timed_out,
        duration_ms,
        stdout: remote_stream(stdout),
        stderr: remote_stream(stderr),
    })
}

fn remote_stream(stream: Stream) -> RemoteStream {
    RemoteStream {
        text: stream.text,
        total_bytes: stream.total_bytes,
        truncated: stream.truncated,
    }
}

// ---------------------------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------------------------

/// The result and the number of bytes read.
async fn run_read(
    runner: &dyn CommandRunner,
    target: &Target,
    params: &RemoteFileReadParams,
) -> Result<(RemoteFileReadResult, u64), AgentBridgeError> {
    validate_agent(params.agent.as_deref())?;
    let path = normalize_path(&params.path, target.cwd.as_deref())?;

    let nonce = new_nonce();
    let script = read_script(&nonce, &path, READ_MAX_FILE_BYTES);
    let output = runner
        .run(&wrap_for_any_shell(&script), COMMAND_TIMEOUT)
        .await?;
    let outcome = parse_read_output(&nonce, &output.stdout, &output.stderr, READ_MAX_FILE_BYTES)?;

    let session = target.session.clone();
    Ok(match outcome {
        ReadOutcome::NotFound => (RemoteFileReadResult::NotFound { session, path }, 0),
        ReadOutcome::Ok { bytes, sha256, .. } => {
            let size = bytes.len() as u64;
            let result = RemoteFileReadResult::Ok {
                session,
                path,
                size,
                sha256,
                content_base64: BASE64.encode(&bytes),
            };
            (result, size)
        }
    })
}

// ---------------------------------------------------------------------------------------------
// write
// ---------------------------------------------------------------------------------------------

async fn run_write(
    runner: &dyn CommandRunner,
    target: &Target,
    params: &RemoteFileWriteParams,
) -> Result<RemoteFileWriteResult, AgentBridgeError> {
    validate_write(params)?;
    let path = normalize_path(&params.path, target.cwd.as_deref())?;
    if params.content_base64.len() > WRITE_MAX_BASE64_LEN {
        return Err(AgentBridgeError::InvalidParams(format!(
            "content is larger than {WRITE_MAX_BYTES} bytes"
        )));
    }
    let content = BASE64.decode(params.content_base64.trim()).map_err(|_| {
        AgentBridgeError::InvalidParams("content_base64 is not valid base64".to_owned())
    })?;
    if content.len() > WRITE_MAX_BYTES {
        return Err(AgentBridgeError::InvalidParams(format!(
            "content is larger than {WRITE_MAX_BYTES} bytes"
        )));
    }

    let tmp_dir = begin_upload(runner).await?;
    let committed = match stage_upload(runner, &tmp_dir, &content).await {
        Ok(()) => commit_upload(runner, &tmp_dir, &path, &content, &params.expectation).await,
        Err(error) => Err((error, true)),
    };
    let outcome = match committed {
        Ok(outcome) => outcome,
        Err((error, scratch_may_remain)) => {
            if scratch_may_remain {
                remove_upload_dir(runner, &tmp_dir).await;
            }
            return Err(error);
        }
    };

    let expected_sha256 = sha256_hex(&content);
    if !outcome.sha256.is_empty() && outcome.sha256 != expected_sha256 {
        return Err(AgentBridgeError::RemoteFailed(format!(
            "The file was written but its checksum on the server does not match the content that \
             was sent, so it may be corrupt.{}",
            outcome
                .backup_path
                .as_deref()
                .map(|backup| format!(" Its previous content is saved in {backup}."))
                .unwrap_or_default()
        )));
    }
    Ok(RemoteFileWriteResult {
        session: target.session.clone(),
        path,
        bytes: content.len() as u64,
        sha256: expected_sha256,
        backup_path: outcome.backup_path,
        created: outcome.created,
    })
}

async fn begin_upload(runner: &dyn CommandRunner) -> Result<RemoteTmpDir, AgentBridgeError> {
    let output = runner.run(&upload_begin_command(), COMMAND_TIMEOUT).await?;
    ensure_success(&output)?;
    Ok(validate_tmp_dir(&String::from_utf8_lossy(&output.stdout))?)
}

async fn stage_upload(
    runner: &dyn CommandRunner,
    tmp_dir: &RemoteTmpDir,
    content: &[u8],
) -> Result<(), AgentBridgeError> {
    let mut chunks = upload_chunk_commands(tmp_dir, content);
    if chunks.is_empty() {
        // An empty file has no chunks, but the commit script still expects the payload to exist.
        chunks.push(format!(
            ": > {}",
            posix_quote(&format!("{}/{PAYLOAD_FILE_NAME}", tmp_dir.as_str()))
        ));
    }
    for chunk in &chunks {
        let output = runner.run(chunk, COMMAND_TIMEOUT).await?;
        ensure_success(&output)?;
    }
    Ok(())
}

/// Runs the commit script. On failure the flag says whether the upload directory may still be on
/// the server: the script removes it itself whenever it gets to report an error.
async fn commit_upload(
    runner: &dyn CommandRunner,
    tmp_dir: &RemoteTmpDir,
    path: &str,
    content: &[u8],
    expectation: &WriteExpectation,
) -> Result<WriteOutcome, (AgentBridgeError, bool)> {
    let nonce = new_nonce();
    let script = write_commit_script(
        &nonce,
        tmp_dir,
        path,
        expectation,
        content.len(),
        &backup_name(path, Utc::now()),
    );
    let output = runner
        .run(&wrap_for_any_shell(&script), COMMAND_TIMEOUT)
        .await
        .map_err(|error| (error, true))?;
    parse_write_output(&nonce, &output.stdout, &output.stderr).map_err(|error| {
        let scratch_may_remain = match error {
            AgentBridgeError::UnexpectedOutput(_) => true,
            AgentBridgeError::NotRemoteSession
            | AgentBridgeError::UnsupportedShell
            | AgentBridgeError::NotAttached { .. }
            | AgentBridgeError::AttachmentExpired
            | AgentBridgeError::ReadOnlyAttachment
            | AgentBridgeError::SessionBusy
            | AgentBridgeError::OperationRunning
            | AgentBridgeError::Timeout { .. }
            | AgentBridgeError::InvalidParams(_)
            | AgentBridgeError::Conflict(_)
            | AgentBridgeError::RemoteFailed(_)
            | AgentBridgeError::Executor(_)
            | AgentBridgeError::Io(_) => false,
        };
        (error, scratch_may_remain)
    })
}

async fn remove_upload_dir(runner: &dyn CommandRunner, tmp_dir: &RemoteTmpDir) {
    if let Err(err) = runner.run(&cleanup_command(tmp_dir), COMMAND_TIMEOUT).await {
        log::debug!("[Agent Bridge] could not remove the upload directory: {err}");
    }
}

/// For a plain command whose failure is the only thing its output says.
fn ensure_success(output: &RawOutput) -> Result<(), AgentBridgeError> {
    if output.success {
        return Ok(());
    }
    let source = if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    Err(AgentBridgeError::RemoteFailed(format!(
        "A command failed on the server: {}",
        remote_failure_message(source)
    )))
}

// ---------------------------------------------------------------------------------------------
// audit
// ---------------------------------------------------------------------------------------------

/// What a finished request adds to its audit record.
struct Details {
    exit_code: Option<i32>,
    bytes: Option<u64>,
}

impl Details {
    fn exec(exit_code: i32) -> Self {
        Self {
            exit_code: Some(exit_code),
            bytes: None,
        }
    }

    fn bytes(bytes: u64) -> Self {
        Self {
            exit_code: None,
            bytes: Some(bytes),
        }
    }
}

struct Audit<'a> {
    target: &'a Target,
    record: AuditRecord,
    started: Instant,
}

impl<'a> Audit<'a> {
    fn begin(target: &'a Target, action: ActionKind, agent: Option<&str>) -> Self {
        let agent = agent
            .filter(|agent| validate_agent(Some(agent)).is_ok())
            .map(str::to_owned);
        let record = AuditRecord {
            ts_unix: u64::try_from(Utc::now().timestamp()).unwrap_or_default(),
            request_id: target.request_id,
            agent,
            action: action.as_str(),
            session_id: target.session.session_id.clone(),
            host: target.session.host.clone(),
            user: target.session.user.clone(),
            cwd: None,
            command: None,
            path: None,
            exit_code: None,
            result: AuditOutcome::Ok,
            error_code: None,
            duration_ms: 0,
            bytes: None,
        };
        Self {
            target,
            record,
            started: Instant::now(),
        }
    }

    /// Records that the request is about to run. Without a trace nothing runs as root, so a log
    /// that cannot be written refuses the request.
    fn record_start(&mut self) -> Result<(), AgentBridgeError> {
        let Some(dir) = &self.target.audit_dir else {
            return Ok(());
        };
        let started = AuditRecord {
            result: AuditOutcome::Started,
            ..self.record.clone()
        };
        audit::append(dir, &started).map_err(|err| {
            AgentBridgeError::Io(format!(
                "The audit log cannot be written, so the request was not run: {err}"
            ))
        })
    }

    /// Writes the closing record. Failing to write it is logged and does not fail the request,
    /// which has already run.
    fn finish(mut self, outcome: Result<Details, AgentBridgeError>) {
        self.record.duration_ms =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match outcome {
            Ok(details) => {
                self.record.exit_code = details.exit_code;
                self.record.bytes = details.bytes;
            }
            Err(error) => {
                self.record.result = AuditOutcome::Error;
                self.record.error_code =
                    Some(::local_control::ControlError::from(error).code.to_string());
            }
        }
        let Some(dir) = &self.target.audit_dir else {
            return;
        };
        if let Err(err) = audit::append(dir, &self.record) {
            log::warn!("[Agent Bridge] could not write the audit log: {err}");
        }
    }
}

#[cfg(all(test, unix))]
#[path = "ops_tests.rs"]
mod tests;
