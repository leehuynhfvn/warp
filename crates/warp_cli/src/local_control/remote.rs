//! `warpctrl remote`: runs commands, reads and writes files and shows recent output in a remote
//! session that the user has allowed agents to use, with the privileges of that session's shell.
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::{ArgGroup, Args, Subcommand};
use local_control::protocol::{
    ActionKind, ControlError, ErrorCode, RemoteAccess, RemoteExecParams, RemoteExecResult,
    RemoteExecVisibleParams, RemoteExecVisibleResult, RemoteFileReadParams, RemoteFileReadResult,
    RemoteFileWriteParams, RemoteFileWriteResult, RemoteOutputRecentParams,
    RemoteOutputRecentResult, RemoteSessionKind, RemoteSessionListResult, RemoteSessionSummary,
    WriteExpectation,
};

use crate::agent::OutputFormat;
use crate::local_control::commands::send_action;
use crate::local_control::output::{write_json, write_json_line};
use crate::local_control::{EXIT_SUCCESS, TargetArgs};

/// Name this CLI gives itself in the audit log of the app.
const AGENT_NAME: &str = "warpctrl-cli";

/// How long the client waits beyond the command's own timeout: the app waits a little longer than
/// the timeout itself before it answers.
pub(super) const EXEC_CLIENT_MARGIN: Duration = Duration::from_secs(30);

/// Reading and writing a file are several remote commands that only run while the shell is idle.
pub(super) const FILE_CLIENT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub(super) const SESSIONS_CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

const MAX_EXIT_CODE: i32 = 255;

/// Largest file `write` sends, matching what the app accepts.
pub(super) const MAX_WRITE_BYTES: u64 = 512 * 1024;

/// Commands `recent` shows by default, and at most, matching what the app accepts.
pub(super) const RECENT_DEFAULT_COUNT: u32 = 3;
pub(super) const RECENT_MAX_COUNT: u32 = 10;

/// Exit code of `read` for a file that does not exist.
const EXIT_NOT_FOUND: u8 = 1;

/// Exit code of `exec --visible` for a command still running when the wait ended, like the exit
/// code of a command stopped by `timeout`.
const EXIT_STILL_RUNNING: u8 = 124;

/// Seconds a command may run when the caller gives no timeout.
pub(super) const EXEC_DEFAULT_TIMEOUT_SECS: u32 = 120;

/// Commands that act on a remote session an agent may use.
///
/// The session has to be allowed first, from the Warp Command Palette in its pane: "Agent Bridge:
/// Allow agents to control this session". Everything runs on the server, not on this machine.
#[derive(Debug, Clone, Subcommand)]
pub enum RemoteCommand {
    /// List the sessions and which of them agents may use.
    Sessions(TargetArgs),

    /// Run a command on the server and print its output.
    ///
    /// Exits with the exit code of the command (124 when it timed out, or with `--visible` when it
    /// is still running). Without `--visible` the command has no terminal and no stdin.
    Exec(RemoteExecArgs),

    /// Print a file from the server.
    Read(RemoteReadArgs),

    /// Replace or create a file on the server with the content of a local file.
    ///
    /// An existing file is only replaced if it still has the checksum given with
    /// `--expected-sha256` (the app saves a backup on the server first); `--create` only creates
    /// a file that does not exist.
    Write(RemoteWriteArgs),

    /// Print the latest commands the user ran in the session, with their output.
    ///
    /// Read from Warp's blocks: nothing runs on the server.
    Recent(RemoteRecentArgs),
}

#[derive(Debug, Clone, Args)]
pub struct RemoteExecArgs {
    /// The session to run in, as listed by `remote sessions`.
    #[command(flatten)]
    pub target: TargetArgs,

    /// Absolute directory to run in. Defaults to the current directory of the session.
    #[arg(long = "cwd")]
    pub cwd: Option<String>,

    /// Type the command into the session's shell, where it runs as a block the user sees.
    ///
    /// `cd` and `export` stay in effect, the user can answer prompts in the terminal, and the
    /// output is what the terminal shows (stdout and stderr together). A command still running
    /// after `--timeout` keeps running in the terminal.
    #[arg(long = "visible", conflicts_with = "cwd")]
    pub visible: bool,

    /// Seconds before the command is stopped, or with `--visible` before warpctrl stops waiting
    /// for it (1-600).
    #[arg(long = "timeout", default_value_t = EXEC_DEFAULT_TIMEOUT_SECS)]
    pub timeout_secs: u32,

    /// The command, joined with spaces and run by the server's shell.
    #[arg(
        required = true,
        num_args = 1..,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteReadArgs {
    #[command(flatten)]
    pub target: TargetArgs,

    /// Absolute path on the server, or one relative to the session's current directory.
    pub path: String,
}

#[derive(Debug, Clone, Args)]
#[command(group(
    ArgGroup::new("expectation")
        .required(true)
        .args(["expected_sha256", "create"])
))]
pub struct RemoteWriteArgs {
    #[command(flatten)]
    pub target: TargetArgs,

    /// Absolute path on the server, or one relative to the session's current directory.
    pub path: String,

    /// Local file whose content is written.
    #[arg(long = "from")]
    pub from: PathBuf,

    /// Overwrite the existing file only if it has this SHA-256 (as printed by `read --output-format
    /// json`).
    #[arg(long = "expected-sha256")]
    pub expected_sha256: Option<String>,

    /// Create the file, failing if it already exists.
    #[arg(long = "create")]
    pub create: bool,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteRecentArgs {
    #[command(flatten)]
    pub target: TargetArgs,

    /// How many of the latest commands to show (1-10).
    #[arg(
        long = "count",
        default_value_t = RECENT_DEFAULT_COUNT,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(RECENT_MAX_COUNT))
    )]
    pub count: u32,
}

pub(super) fn run_remote_command(
    command: RemoteCommand,
    output_format: OutputFormat,
) -> Result<u8, ControlError> {
    match command {
        RemoteCommand::Sessions(args) => run_sessions(&args, output_format),
        RemoteCommand::Exec(args) => run_exec(args, output_format),
        RemoteCommand::Read(args) => run_read(&args, output_format),
        RemoteCommand::Write(args) => run_write(args, output_format),
        RemoteCommand::Recent(args) => run_recent(&args, output_format),
    }
}

fn run_sessions(args: &TargetArgs, output_format: OutputFormat) -> Result<u8, ControlError> {
    let data = send_action(
        args,
        ActionKind::RemoteSessionList,
        serde_json::json!({}),
        SESSIONS_CLIENT_TIMEOUT,
    )?;
    let result: RemoteSessionListResult = decode(data.clone(), "session list")?;
    print_result(&data, output_format, || {
        println!("{}", render_sessions(&result.sessions));
        Ok(())
    })?;
    Ok(EXIT_SUCCESS)
}

fn run_exec(args: RemoteExecArgs, output_format: OutputFormat) -> Result<u8, ControlError> {
    if args.visible {
        return run_exec_visible(args, output_format);
    }
    let params = RemoteExecParams {
        command: args.command.join(" "),
        cwd: args.cwd,
        timeout_secs: Some(args.timeout_secs),
        agent: Some(AGENT_NAME.to_owned()),
    };
    let wait = Duration::from_secs(args.timeout_secs.into()) + EXEC_CLIENT_MARGIN;
    let data = send_action(&args.target, ActionKind::RemoteExec, params, wait)?;
    let result: RemoteExecResult = decode(data.clone(), "command result")?;
    print_result(&data, output_format, || {
        print_exec_output(&result);
        Ok(())
    })?;
    Ok(exec_exit_code(&result))
}

fn run_exec_visible(args: RemoteExecArgs, output_format: OutputFormat) -> Result<u8, ControlError> {
    let params = RemoteExecVisibleParams {
        command: args.command.join(" "),
        timeout_secs: Some(args.timeout_secs),
        agent: Some(AGENT_NAME.to_owned()),
    };
    let wait = Duration::from_secs(args.timeout_secs.into()) + EXEC_CLIENT_MARGIN;
    let data = send_action(&args.target, ActionKind::RemoteExecVisible, params, wait)?;
    let result: RemoteExecVisibleResult = decode(data.clone(), "command result")?;
    print_result(&data, output_format, || {
        print!("{}", terminal_safe(&result.output));
        eprintln!("{}", render_visible_status(&result));
        Ok(())
    })?;
    Ok(visible_exit_code(&result))
}

fn run_read(args: &RemoteReadArgs, output_format: OutputFormat) -> Result<u8, ControlError> {
    let params = RemoteFileReadParams {
        path: args.path.clone(),
        agent: Some(AGENT_NAME.to_owned()),
    };
    let data = send_action(
        &args.target,
        ActionKind::RemoteFileRead,
        params,
        FILE_CLIENT_TIMEOUT,
    )?;
    let result: RemoteFileReadResult = decode(data.clone(), "file content")?;
    let content = match result {
        RemoteFileReadResult::NotFound { path, .. } => {
            return print_result(&data, output_format, || {
                eprintln!("error: {path} does not exist on the server");
                Ok(())
            })
            .map(|()| EXIT_NOT_FOUND);
        }
        RemoteFileReadResult::Ok { content_base64, .. } => {
            BASE64.decode(content_base64).map_err(|err| {
                ControlError::with_details(
                    ErrorCode::Internal,
                    "the file content is not valid base64",
                    err.to_string(),
                )
            })?
        }
    };
    print_result(&data, output_format, || write_stdout(&content))?;
    Ok(EXIT_SUCCESS)
}

fn run_write(args: RemoteWriteArgs, output_format: OutputFormat) -> Result<u8, ControlError> {
    let unreadable = |err: std::io::Error| {
        ControlError::with_details(
            ErrorCode::InvalidParams,
            format!("cannot read {}", args.from.display()),
            err.to_string(),
        )
    };
    let size = std::fs::metadata(&args.from).map_err(unreadable)?.len();
    if size > MAX_WRITE_BYTES {
        return Err(ControlError::new(
            ErrorCode::InvalidParams,
            format!(
                "{} is {size} bytes; at most {MAX_WRITE_BYTES} can be written in one go",
                args.from.display()
            ),
        ));
    }
    let content = std::fs::read(&args.from).map_err(|err| {
        ControlError::with_details(
            ErrorCode::InvalidParams,
            format!("cannot read {}", args.from.display()),
            err.to_string(),
        )
    })?;
    let expectation = match args.expected_sha256 {
        Some(sha256) => WriteExpectation::MustMatch {
            sha256: sha256.to_lowercase(),
        },
        None => WriteExpectation::MustNotExist,
    };
    let params = RemoteFileWriteParams {
        path: args.path,
        content_base64: BASE64.encode(content),
        expectation,
        agent: Some(AGENT_NAME.to_owned()),
    };
    let data = send_action(
        &args.target,
        ActionKind::RemoteFileWrite,
        params,
        FILE_CLIENT_TIMEOUT,
    )?;
    let result: RemoteFileWriteResult = decode(data.clone(), "write result")?;
    print_result(&data, output_format, || {
        println!("{}", render_write(&result));
        Ok(())
    })?;
    Ok(EXIT_SUCCESS)
}

fn run_recent(args: &RemoteRecentArgs, output_format: OutputFormat) -> Result<u8, ControlError> {
    let params = RemoteOutputRecentParams {
        count: Some(args.count),
        agent: Some(AGENT_NAME.to_owned()),
    };
    let data = send_action(
        &args.target,
        ActionKind::RemoteOutputRecent,
        params,
        SESSIONS_CLIENT_TIMEOUT,
    )?;
    let result: RemoteOutputRecentResult = decode(data.clone(), "command history")?;
    print_result(&data, output_format, || {
        println!("{}", terminal_safe(&render_recent(&result)));
        Ok(())
    })?;
    Ok(EXIT_SUCCESS)
}

/// A result this CLI cannot read (for example from a Warp of another version) must not be taken
/// for success.
pub(super) fn decode<T: serde::de::DeserializeOwned>(
    data: serde_json::Value,
    what: &str,
) -> Result<T, ControlError> {
    serde_json::from_value(data).map_err(|err| {
        ControlError::with_details(
            ErrorCode::Internal,
            format!(
                "Warp answered with a {what} this warpctrl does not understand; are they the same \
                 version?"
            ),
            err.to_string(),
        )
    })
}

fn print_result(
    data: &serde_json::Value,
    output_format: OutputFormat,
    print_text: impl FnOnce() -> Result<(), ControlError>,
) -> Result<(), ControlError> {
    match output_format {
        OutputFormat::Json => write_json(data),
        OutputFormat::Ndjson => write_json_line(data),
        OutputFormat::Pretty | OutputFormat::Text => print_text(),
    }
}

/// Writes raw bytes to stdout. A closed pipe (`| head`) is the reader's choice, not a failure.
fn write_stdout(bytes: &[u8]) -> Result<(), ControlError> {
    match std::io::stdout().write_all(bytes) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(ControlError::with_details(
            ErrorCode::Internal,
            "failed to write local-control output",
            err.to_string(),
        )),
    }
}

fn print_exec_output(result: &RemoteExecResult) {
    print!("{}", terminal_safe(&result.stdout.text));
    eprint!("{}", terminal_safe(&result.stderr.text));
    for (name, stream) in [("stdout", &result.stdout), ("stderr", &result.stderr)] {
        if stream.truncated {
            eprintln!(
                "warpctrl: {name} was cut; the command printed {} bytes",
                stream.total_bytes
            );
        }
    }
    if result.timed_out {
        eprintln!("warpctrl: the command timed out and was stopped");
    }
}

/// `text` without the characters that a terminal would obey (escape sequences), so that what a
/// server prints cannot rewrite the operator's screen. Tabs, newlines and carriage returns stay.
pub(super) fn terminal_safe(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t' | '\r'))
        .collect()
}

/// How a visible command ended, for stderr after its output.
pub(super) fn render_visible_status(result: &RemoteExecVisibleResult) -> String {
    let mut status = match (result.exit_code, result.alt_screen) {
        (Some(exit_code), _) => format!("warpctrl: exit {exit_code}"),
        (None, true) => "warpctrl: still running in the terminal (full-screen program)".to_owned(),
        (None, false) => "warpctrl: still running in the terminal".to_owned(),
    };
    if result.truncated {
        status.push_str(&format!(
            "\nwarpctrl: output was cut; the whole output is {} rows",
            result.output_rows
        ));
    }
    status
}

pub(super) fn visible_exit_code(result: &RemoteExecVisibleResult) -> u8 {
    match result.exit_code {
        Some(exit_code) => u8::try_from(exit_code.clamp(0, MAX_EXIT_CODE)).unwrap_or(u8::MAX),
        None => EXIT_STILL_RUNNING,
    }
}

/// The exit code of the command, so that scripts can use `warpctrl remote exec` like `ssh`.
pub(super) fn exec_exit_code(result: &RemoteExecResult) -> u8 {
    u8::try_from(result.exit_code.clamp(0, MAX_EXIT_CODE)).unwrap_or(u8::MAX)
}

pub(super) fn render_sessions(sessions: &[RemoteSessionSummary]) -> String {
    if sessions.is_empty() {
        return "No sessions.".to_owned();
    }
    sessions
        .iter()
        .map(render_session)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_session(session: &RemoteSessionSummary) -> String {
    let marker = if session.is_active { '*' } else { ' ' };
    let place = match session.session_type {
        RemoteSessionKind::Remote => format!("{}@{}", session.user, session.host),
        RemoteSessionKind::Local => "(local)".to_owned(),
    };
    let cwd = session.cwd.as_deref().unwrap_or("-");
    let access = match &session.attached {
        None => "not attached".to_owned(),
        Some(attached) => {
            let access = match attached.access {
                RemoteAccess::Full => "full access",
                RemoteAccess::ReadOnly => "read-only",
            };
            format!(
                "{access}, {} commands, expires in {}",
                attached.exec_count,
                minutes(attached.expires_in_secs)
            )
        }
    };
    format!(
        "{marker} {}  {place}  {}  {cwd}  [{access}]",
        session.session_id, session.shell
    )
}

fn minutes(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        secs => format!("{}m", secs / 60),
    }
}

pub(super) fn render_write(result: &RemoteFileWriteResult) -> String {
    let verb = if result.created { "Created" } else { "Wrote" };
    let mut text = format!(
        "{verb} {} ({} bytes, sha256 {})",
        result.path, result.bytes, result.sha256
    );
    if let Some(backup) = &result.backup_path {
        text.push_str(&format!("\nPrevious content saved in {backup}"));
    }
    text
}

pub(super) fn render_recent(result: &RemoteOutputRecentResult) -> String {
    let session = &result.session;
    if result.blocks.is_empty() {
        return format!(
            "No finished commands in {}@{} yet.",
            session.user, session.host
        );
    }
    let mut text = format!(
        "Latest commands the user ran in {}@{}, oldest first:",
        session.user, session.host
    );
    for block in &result.blocks {
        let cwd = block
            .cwd
            .as_deref()
            .map(|cwd| format!(", in {cwd}"))
            .unwrap_or_default();
        text.push_str(&format!(
            "\n\n$ {}\n[exit {}{cwd}]\n{}",
            block.command.trim_end(),
            block.exit_code,
            block.output.trim_end()
        ));
        if block.truncated {
            text.push_str(&format!(
                "\n[output cut; the whole output is {} rows]",
                block.output_rows
            ));
        }
    }
    text
}
