//! Builders for the shell scripts the Agent Bridge runs on the remote host, and parsers for their
//! output. Scripts are POSIX `sh` (wrap them with `wrap_for_any_shell` to run them from any
//! interactive shell). They always exit 0 and report through lines that start with a per-request
//! marker, so that a failure of the operation can be told apart from a failure of the transport.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Utc};
use ::local_control::protocol::WriteExpectation;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::error::AgentBridgeError;
use super::{EXEC_STDERR_MAX_BYTES, EXEC_STDOUT_MAX_BYTES};
use crate::warp_sync::remote_script::{PAYLOAD_FILE_NAME, RemoteTmpDir, posix_quote};

const NONCE_LEN: usize = 16;
const UNEXPECTED_OUTPUT_TAIL_BYTES: usize = 200;
const MAX_BACKUP_BASENAME_CHARS: usize = 100;
const BACKUP_SUFFIX_CHARS: usize = 4;
const BACKUP_TIMESTAMP_FORMAT: &str = "%Y%m%dT%H%M%S";

/// Exit status `timeout` uses when it kills the command.
const TIMEOUT_EXIT_CODE: i32 = 124;

/// Remote directory (relative to `$HOME`) that receives a backup of every overwritten file. It is
/// outside the file's directory so that config globs such as `conf.d/*.conf` cannot load it.
const BACKUP_DIR: &str = "$HOME/.warp-agent/backups";

/// A random string that makes the marker of one request unguessable to the previous ones.
pub(crate) fn new_nonce() -> String {
    Uuid::new_v4().simple().to_string()[..NONCE_LEN].to_owned()
}

fn marker(nonce: &str) -> String {
    format!("@@WARP-AGENT-{nonce}@@")
}

/// The remainder of `line` after the marker, if `line` is a framing line.
fn frame_field<'a>(marker: &str, line: &'a str) -> Option<&'a str> {
    line.strip_prefix(marker)?
        .strip_prefix(' ')
        .map(|rest| rest.trim_end_matches('\r'))
}

fn unexpected(message: impl Into<String>) -> AgentBridgeError {
    AgentBridgeError::UnexpectedOutput(message.into())
}

fn last_bytes(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// The output that carries the framing. The in-band executor usually puts it on stdout, but a
/// failed command's output may be on stderr.
fn framed_text(marker: &str, stdout: &[u8], stderr: &[u8]) -> Result<String, AgentBridgeError> {
    let stdout = String::from_utf8_lossy(stdout);
    if stdout.contains(marker) {
        return Ok(stdout.into_owned());
    }
    let stderr = String::from_utf8_lossy(stderr);
    if stderr.contains(marker) {
        return Ok(stderr.into_owned());
    }
    let seen = if stdout.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    Err(unexpected(last_bytes(seen, UNEXPECTED_OUTPUT_TAIL_BYTES)))
}

/// Byte offset of the first line of `text` that starts with `prefix`.
fn find_line(text: &str, prefix: &str) -> Option<usize> {
    text.match_indices(prefix)
        .map(|(index, _)| index)
        .find(|&index| index == 0 || text.as_bytes()[index - 1] == b'\n')
}

// ---------------------------------------------------------------------------------------------
// exec
// ---------------------------------------------------------------------------------------------

/// Script that runs `command` in a fresh shell with stdin closed, capturing stdout and stderr in
/// scratch files so that long output can be cut to its head and tail.
pub(crate) fn exec_script(
    nonce: &str,
    command: &str,
    cwd: Option<&str>,
    timeout_secs: u32,
) -> String {
    let marker = posix_quote(&marker(nonce));
    let command = posix_quote(command);
    let cd_step = cwd
        .map(|cwd| {
            let cwd = posix_quote(cwd);
            format!(r#"cd {cwd} 2>/dev/null || {{ rm -rf "$T"; echo "$M fatal cwd"; exit 0; }}"#)
        })
        .unwrap_or_default();
    let stdout_max = EXEC_STDOUT_MAX_BYTES;
    let stdout_half = EXEC_STDOUT_MAX_BYTES / 2;
    let stderr_max = EXEC_STDERR_MAX_BYTES;
    let stderr_half = EXEC_STDERR_MAX_BYTES / 2;
    format!(
        r#"M={marker}
T=$(mktemp -d "${{TMPDIR:-/tmp}}/warp-agent.XXXXXX") || {{ echo "$M fatal mktemp"; exit 0; }}
{cd_step}
export PAGER=cat GIT_PAGER=cat SYSTEMD_PAGER=cat NO_COLOR=1 DEBIAN_FRONTEND=noninteractive
C={command}
if command -v bash >/dev/null 2>&1; then S=bash; else S=sh; fi
if command -v timeout >/dev/null 2>&1; then
  timeout {timeout_secs} "$S" -c "$C" </dev/null >"$T/o" 2>"$T/e"; rc=$?; TO=1
else
  "$S" -c "$C" </dev/null >"$T/o" 2>"$T/e"; rc=$?; TO=0
fi
emit() {{
  n=$(wc -c < "$2" | tr -d ' ')
  echo "$M $1 $n"
  if [ "$n" -le "$3" ]; then cat "$2"; else head -c "$4" "$2"; printf '\n%s cut\n' "$M"; tail -c "$4" "$2"; fi
  printf '\n%s end\n' "$M"
}}
emit stdout "$T/o" {stdout_max} {stdout_half}
emit stderr "$T/e" {stderr_max} {stderr_half}
echo "$M rc $rc $TO"
rm -rf "$T"
exit 0
"#
    )
}

/// One captured output stream of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stream {
    /// The output, lossily decoded. When `truncated`, the middle is replaced by a note.
    pub text: String,
    pub total_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecOutput {
    pub stdout: Stream,
    pub stderr: Stream,
    pub exit_code: i32,
    pub timed_out: bool,
}

pub(crate) fn parse_exec_output(
    nonce: &str,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<ExecOutput, AgentBridgeError> {
    let marker = marker(nonce);
    let text = framed_text(&marker, stdout, stderr)?;
    if let Some(start) = find_line(&text, &format!("{marker} fatal ")) {
        return Err(fatal_error(&marker, &text[start..]));
    }
    let (stdout, rest) = parse_stream(&marker, "stdout", EXEC_STDOUT_MAX_BYTES, &text)?;
    let (stderr, rest) = parse_stream(&marker, "stderr", EXEC_STDERR_MAX_BYTES, rest)?;
    let rc_prefix = format!("{marker} rc ");
    let start = find_line(rest, &rc_prefix).ok_or_else(|| unexpected("no exit status"))?;
    let line = rest[start + rc_prefix.len()..].lines().next().unwrap_or_default();
    let mut fields = line.split_whitespace();
    let exit_code: i32 = fields
        .next()
        .and_then(|field| field.parse().ok())
        .ok_or_else(|| unexpected("malformed exit status"))?;
    let has_timeout = fields.next() == Some("1");
    Ok(ExecOutput {
        stdout,
        stderr,
        exit_code,
        timed_out: has_timeout && exit_code == TIMEOUT_EXIT_CODE,
    })
}

fn fatal_error(marker: &str, from_fatal_line: &str) -> AgentBridgeError {
    let line = from_fatal_line.lines().next().unwrap_or_default();
    match frame_field(marker, line).and_then(|rest| rest.strip_prefix("fatal ")) {
        Some("cwd") => {
            AgentBridgeError::InvalidParams("cwd does not exist or is not accessible".to_owned())
        }
        Some("mktemp") => AgentBridgeError::RemoteFailed(
            "could not create a scratch directory on the server".to_owned(),
        ),
        Some(other) => unexpected(format!("the script stopped: {other}")),
        None => unexpected("malformed failure report"),
    }
}

/// Reads the `name` section, returning it and the text after it.
fn parse_stream<'a>(
    marker: &str,
    name: &str,
    max_bytes: usize,
    text: &'a str,
) -> Result<(Stream, &'a str), AgentBridgeError> {
    let header = format!("{marker} {name} ");
    let start = find_line(text, &header).ok_or_else(|| unexpected(format!("no {name} section")))?;
    let after_header = start + header.len();
    let header_end = text[after_header..]
        .find('\n')
        .map(|offset| after_header + offset)
        .ok_or_else(|| unexpected(format!("unterminated {name} header")))?;
    let total_bytes: u64 = text[after_header..header_end]
        .trim()
        .parse()
        .map_err(|_| unexpected(format!("malformed {name} size")))?;

    let body_start = header_end + 1;
    let end_delimiter = format!("\n{marker} end\n");
    let end = text[body_start..]
        .find(&end_delimiter)
        .ok_or_else(|| unexpected(format!("unterminated {name} section")))?;
    let body = &text[body_start..body_start + end];
    let rest = &text[body_start + end + end_delimiter.len()..];

    let cut_delimiter = format!("\n{marker} cut\n");
    let stream = match body.split_once(&cut_delimiter) {
        Some((head, tail)) => {
            let kept = 2 * (max_bytes / 2) as u64;
            let omitted = total_bytes.saturating_sub(kept);
            Stream {
                text: format!("{head}\n… [{omitted} bytes omitted] …\n{tail}"),
                total_bytes,
                truncated: true,
            }
        }
        None => Stream {
            text: body.to_owned(),
            total_bytes,
            truncated: false,
        },
    };
    Ok((stream, rest))
}

// ---------------------------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------------------------

/// Script that prints `path` base64-encoded, after checking that it is a readable regular file of
/// at most `max_bytes`.
pub(crate) fn read_script(nonce: &str, path: &str, max_bytes: usize) -> String {
    let marker = posix_quote(&marker(nonce));
    let path = posix_quote(path);
    format!(
        r#"M={marker}; P={path}
echo "$M user $(id -un 2>/dev/null)"
if [ ! -e "$P" ]; then echo "$M status not_found"; exit 0; fi
if [ ! -f "$P" ]; then echo "$M status not_regular"; exit 0; fi
if [ ! -r "$P" ]; then echo "$M status permission_denied"; exit 0; fi
if ! command -v base64 >/dev/null 2>&1; then echo "$M status missing_base64"; exit 0; fi
n=$(wc -c < "$P" | tr -d ' ')
echo "$M size $n"
if [ "$n" -gt {max_bytes} ]; then echo "$M status too_large"; exit 0; fi
echo "$M status ok"
base64 < "$P"
printf '\n%s end\n' "$M"
exit 0
"#
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReadOutcome {
    NotFound,
    Ok {
        bytes: Vec<u8>,
        /// Lowercase hex, computed here from `bytes` so that it does not depend on the server
        /// having a hashing tool.
        sha256: String,
        user: String,
    },
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn parse_read_output(
    nonce: &str,
    stdout: &[u8],
    stderr: &[u8],
    max_bytes: usize,
) -> Result<ReadOutcome, AgentBridgeError> {
    let marker = marker(nonce);
    let text = framed_text(&marker, stdout, stderr)?;
    let mut lines = text.lines();
    let mut user = String::new();
    let mut size: Option<u64> = None;
    let mut status = None;
    for line in lines.by_ref() {
        let Some(field) = frame_field(&marker, line) else {
            continue;
        };
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        match key {
            "user" => user = value.to_owned(),
            "size" => size = value.parse().ok(),
            "status" => {
                status = Some(value);
                break;
            }
            _ => {}
        }
    }
    match status {
        Some("ok") => {}
        Some("not_found") => return Ok(ReadOutcome::NotFound),
        Some(other) => return Err(read_status_error(other, &user, size, max_bytes)),
        None => return Err(unexpected("no status")),
    }

    let mut encoded = String::new();
    let mut complete = false;
    for line in lines {
        if frame_field(&marker, line) == Some("end") {
            complete = true;
            break;
        }
        encoded.push_str(line.trim());
    }
    if !complete {
        return Err(unexpected("the file content was cut short"));
    }
    let bytes = BASE64
        .decode(encoded.as_bytes())
        .map_err(|_| unexpected("the file content is not valid base64"))?;
    if bytes.len() > max_bytes {
        return Err(too_large(bytes.len() as u64, max_bytes));
    }
    if size.is_some_and(|size| size != bytes.len() as u64) {
        return Err(AgentBridgeError::Conflict(
            "The file changed while it was being read; try again".to_owned(),
        ));
    }
    Ok(ReadOutcome::Ok {
        sha256: sha256_hex(&bytes),
        bytes,
        user,
    })
}

fn too_large(size: u64, max_bytes: usize) -> AgentBridgeError {
    AgentBridgeError::RemoteFailed(format!(
        "The file is {size} bytes, over the {max_bytes}-byte limit. Use exec with tail, head or \
         grep to look at part of it"
    ))
}

fn read_status_error(
    status: &str,
    user: &str,
    size: Option<u64>,
    max_bytes: usize,
) -> AgentBridgeError {
    match status {
        "not_regular" => AgentBridgeError::RemoteFailed("The path is not a regular file".to_owned()),
        "permission_denied" => {
            AgentBridgeError::RemoteFailed(format!("Permission denied (running as {user})"))
        }
        "missing_base64" => {
            AgentBridgeError::RemoteFailed("The server has no `base64` command".to_owned())
        }
        "too_large" => too_large(size.unwrap_or_default(), max_bytes),
        other => unexpected(format!("unknown read status: {other}")),
    }
}

// ---------------------------------------------------------------------------------------------
// write
// ---------------------------------------------------------------------------------------------

/// Script that puts the staged payload in `tmp_dir` at `path`, provided `expectation` holds.
///
/// An existing file is overwritten in place (never replaced by a new inode) so that its owner,
/// mode, ACLs, SELinux label and hard links survive, after its current content was copied to the
/// backup directory. A new file is created with `noclobber`, which fails instead of following a
/// symlink or replacing a file that appeared in the meantime.
pub(crate) fn write_commit_script(
    nonce: &str,
    tmp_dir: &RemoteTmpDir,
    path: &str,
    expectation: &WriteExpectation,
    expected_len: usize,
    backup_name: &str,
) -> String {
    let marker = posix_quote(&marker(nonce));
    let tmp_dir = posix_quote(tmp_dir.as_str());
    let path = posix_quote(path);
    let step = match expectation {
        WriteExpectation::MustNotExist => create_step(),
        WriteExpectation::MustMatch { sha256 } => overwrite_step(sha256, backup_name),
    };
    format!(
        r#"M={marker}; T={tmp_dir}; P={path}; F="$T/{PAYLOAD_FILE_NAME}"
h() {{ if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1; fi; }}
fail() {{ rm -rf "$T"; echo "$M error $1"; exit 0; }}
[ "$(wc -c < "$F" | tr -d ' ')" = "{expected_len}" ] || fail size_mismatch
{step}
rm -rf "$T"
echo "$M sha256 $(h "$P")"
echo "$M status ok"
exit 0
"#
    )
}

fn create_step() -> String {
    r#"if [ -e "$P" ] || [ -L "$P" ]; then fail already_exists; fi
[ -d "$(dirname "$P")" ] || fail parent_missing
if ! ( set -C; cat "$F" > "$P" ) 2>/dev/null; then
  if [ -e "$P" ] || [ -L "$P" ]; then fail already_exists; fi
  fail write_failed
fi
echo "$M created 1""#
        .to_owned()
}

fn overwrite_step(sha256: &str, backup_name: &str) -> String {
    let sha256 = posix_quote(sha256);
    let backup_file = posix_quote(backup_name);
    format!(
        r#"[ -e "$P" ] || fail not_found
[ -f "$P" ] || fail not_regular
cur=$(h "$P"); [ -n "$cur" ] || fail missing_sha256
[ "$cur" = {sha256} ] || fail changed_on_server
B="{BACKUP_DIR}"
if [ -z "$HOME" ] || [ ! -d "$HOME" ] || [ -L "$HOME/.warp-agent" ] || [ -L "$B" ]; then fail backup_failed; fi
mkdir -p "$B" && chmod 700 "$HOME/.warp-agent" "$B" || fail backup_failed
cp -p "$P" "$B/"{backup_file} || fail backup_failed
echo "$M backup $B/"{backup_file}
cat "$F" > "$P" || fail write_failed"#
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteOutcome {
    pub created: bool,
    /// Where the previous content was saved; absent when the file is new.
    pub backup_path: Option<String>,
    /// Checksum of the file as the server sees it after the write.
    pub sha256: String,
}

pub(crate) fn parse_write_output(
    nonce: &str,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<WriteOutcome, AgentBridgeError> {
    let marker = marker(nonce);
    let text = framed_text(&marker, stdout, stderr)?;
    let mut created = false;
    let mut backup_path = None;
    let mut sha256 = String::new();
    let mut error = None;
    let mut done = false;
    for field in text.lines().filter_map(|line| frame_field(&marker, line)) {
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        match key {
            "created" => created = true,
            "backup" => backup_path = Some(value.to_owned()),
            "sha256" => sha256 = value.trim().to_owned(),
            "error" => error = Some(value.to_owned()),
            "status" => done = value == "ok",
            _ => {}
        }
    }
    if let Some(code) = error {
        return Err(write_error(&code, backup_path.as_deref()));
    }
    if !done {
        return Err(unexpected("the write did not report completion"));
    }
    Ok(WriteOutcome {
        created,
        backup_path,
        sha256,
    })
}

fn write_error(code: &str, backup_path: Option<&str>) -> AgentBridgeError {
    let failed = |message: &str| AgentBridgeError::RemoteFailed(message.to_owned());
    match code {
        "already_exists" => AgentBridgeError::Conflict(
            "The file already exists on the server. Read it and write with its checksum instead"
                .to_owned(),
        ),
        "changed_on_server" => AgentBridgeError::Conflict(
            "The file changed on the server since it was read. Read it again and redo the change"
                .to_owned(),
        ),
        "not_found" => failed("The file no longer exists on the server"),
        "not_regular" => failed("The path is not a regular file"),
        "parent_missing" => failed("The parent directory does not exist on the server"),
        "missing_sha256" => failed("The server has neither `sha256sum` nor `shasum`"),
        "backup_failed" => failed(
            "Could not save a backup under ~/.warp-agent/backups on the server; nothing was written",
        ),
        "size_mismatch" => failed("The content was corrupted while it was sent to the server"),
        "write_failed" => match backup_path {
            Some(backup) => AgentBridgeError::RemoteFailed(format!(
                "Writing the file failed and it may be incomplete. Its previous content is saved \
                 in {backup}"
            )),
            None => failed("Writing the file failed"),
        },
        other => unexpected(format!("unknown write error: {other}")),
    }
}

/// A file name for the backup of `path` that identifies the file and the time, using only
/// characters that are safe in a shell word. Uniqueness comes from the random suffix.
pub(crate) fn backup_name(path: &str, now: DateTime<Utc>) -> String {
    let basename = path.rsplit('/').next().unwrap_or_default();
    let stem: String = basename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_BACKUP_BASENAME_CHARS)
        .collect();
    let stem = if stem.is_empty() { "file" } else { &stem };
    let suffix = &Uuid::new_v4().simple().to_string()[..BACKUP_SUFFIX_CHARS];
    format!("{stem}.{}.{suffix}", now.format(BACKUP_TIMESTAMP_FORMAT))
}

#[cfg(test)]
#[path = "script_tests.rs"]
mod tests;
