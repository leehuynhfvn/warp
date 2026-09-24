//! Builders for the shell commands Warp Sync runs on the remote host, and parsers for their
//! output. Scripts are POSIX `sh`; [`wrap_for_any_shell`] makes them runnable from whatever
//! interactive shell the session uses.

use std::sync::LazyLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use regex::Regex;

use super::{UPLOAD_CHUNK_B64_LEN, WarpSyncError};

const FAILURE_MESSAGE_TAIL_BYTES: usize = 1024;
const MIN_MACHINE_ID_LEN: usize = 8;
const MAX_MACHINE_ID_LEN: usize = 64;

/// Name of the payload file inside the remote temporary directory.
const PAYLOAD_FILE_NAME: &str = "payload.tgz";

/// Remote directory (relative to `$HOME`) that receives a backup of every overwritten target.
/// It lives outside the target so that config globs such as `conf.d/*.conf` cannot load it.
const BACKUP_DIR: &str = "$HOME/.warp-sync/backups";

const EXIT_SIZE_MISMATCH: i32 = 81;
const EXIT_CORRUPT_PAYLOAD: i32 = 82;
const EXIT_BACKUP_DIR: i32 = 83;
const EXIT_BACKUP_FAILED: i32 = 84;
const EXIT_EXTRACT_FAILED: i32 = 85;

static TMP_DIR_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/[A-Za-z0-9._/-]+/warp-sync\.[A-Za-z0-9]+$").expect("static regex is valid")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeStatus {
    Ok,
    NotFound,
    PermissionDenied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteKind {
    File,
    Dir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarFlavor {
    Gnu,
    Other,
}

/// What the remote host reported about a path and about itself. Only `status` is meaningful when
/// it is [`ProbeStatus::NotFound`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub status: ProbeStatus,
    pub user: String,
    pub uid: u32,
    pub kind: RemoteKind,
    /// `None` when `du` could not measure the path, e.g. because part of it is unreadable.
    pub size_kib: Option<u64>,
    pub tar: TarFlavor,
    pub has_base64: bool,
    /// The remote host's machine id, which unlike its hostname is not chosen by whoever
    /// configured the session. `None` when the host has none.
    pub machine_id: Option<String>,
}

/// How `tar -x` should treat ownership on the remote host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractMode {
    /// Root on GNU tar: restore the exact uid/gid.
    GnuPreserveOwner,
    GnuNoOwner,
    /// Non-GNU tar (e.g. BusyBox): ownership may not be restored completely.
    Generic,
}

impl ExtractMode {
    pub fn for_probe(probe: &ProbeResult) -> Self {
        match (probe.tar, probe.uid) {
            (TarFlavor::Gnu, 0) => Self::GnuPreserveOwner,
            (TarFlavor::Gnu, _) => Self::GnuNoOwner,
            (TarFlavor::Other, _) => Self::Generic,
        }
    }

    fn tar_flags(self) -> &'static str {
        match self {
            Self::GnuPreserveOwner => "-p --same-owner --numeric-owner",
            Self::GnuNoOwner => "-p --no-same-owner",
            Self::Generic => "-p",
        }
    }
}

/// A remote scratch directory created by [`upload_begin_command`]. Only obtainable through
/// [`validate_tmp_dir`], so every command that embeds it has a vetted path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTmpDir(String);

impl RemoteTmpDir {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct UploadCommit<'a> {
    pub tmp_dir: &'a RemoteTmpDir,
    pub parent: &'a str,
    pub name: &'a str,
    pub expected_len: usize,
    /// File-name stem of the backup; must only contain `[A-Za-z0-9._-]`.
    pub backup_name: &'a str,
    pub extract_mode: ExtractMode,
}

/// Quotes `s` as a single POSIX shell word.
pub fn posix_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Turns a POSIX `sh` script into a one-line command that any shell (bash, zsh, fish) runs
/// identically. The base64 alphabet needs no quoting in any of them.
pub fn wrap_for_any_shell(script: &str) -> String {
    format!("printf %s {} | base64 -d | sh", BASE64.encode(script))
}

/// Script that reports `key=value` lines about `path`. Always exits 0.
pub fn probe_script(path: &str) -> String {
    let path = posix_quote(path);
    format!(
        r#"P={path}
if [ ! -e "$P" ]; then echo status=not_found; exit 0; fi
if [ ! -r "$P" ]; then echo status=permission_denied; else echo status=ok; fi
echo "user=$(id -un)"; echo "uid=$(id -u)"
if [ -d "$P" ]; then echo kind=dir; else echo kind=file; fi
echo "size_kib=$(du -sk "$P" 2>/dev/null | cut -f1)"
if tar --version 2>/dev/null | grep -q GNU; then echo tar=gnu; else echo tar=other; fi
if command -v base64 >/dev/null 2>&1; then echo base64=yes; else echo base64=no; fi
echo "machine_id=$(cat /etc/machine-id 2>/dev/null || cat /var/lib/dbus/machine-id 2>/dev/null)"
"#
    )
}

/// Script whose stdout is exactly the gzipped tarball of `parent/name`. A tar failure is printed
/// instead, with tar's exit code.
pub fn download_script(parent: &str, name: &str) -> String {
    let parent = posix_quote(parent);
    // The `./` prefix stops a name starting with `-` from being read as an option.
    let name = posix_quote(&format!("./{name}"));
    format!(
        r#"E=$(mktemp) || exit 90
tar -czf - -C {parent} {name} 2>"$E"; rc=$?
if [ $rc -ne 0 ]; then cat "$E"; fi; rm -f "$E"; exit $rc
"#
    )
}

/// Command that creates a private scratch directory and prints its path.
pub fn upload_begin_command() -> String {
    wrap_for_any_shell(
        r#"T=$(mktemp -d "${TMPDIR:-/tmp}/warp-sync.XXXXXX") && chmod 700 "$T" && echo "$T""#,
    )
}

/// Accepts the output of [`upload_begin_command`] only if it is a plausible scratch directory
/// path, since the path is later embedded in commands (including `rm -rf`).
pub fn validate_tmp_dir(output: &str) -> Result<RemoteTmpDir, WarpSyncError> {
    let path = output.trim();
    let has_dot_dot = path.split('/').any(|component| component == "..");
    if TMP_DIR_REGEX.is_match(path) && !has_dot_dot {
        Ok(RemoteTmpDir(path.to_owned()))
    } else {
        Err(unexpected_output("unexpected scratch directory path"))
    }
}

/// Commands that append `tgz` to the scratch directory's payload file, in order. Each chunk is a
/// standalone base64 string so that it decodes on its own.
pub fn upload_chunk_commands(tmp_dir: &RemoteTmpDir, tgz: &[u8]) -> Vec<String> {
    let payload = posix_quote(&format!("{}/{PAYLOAD_FILE_NAME}", tmp_dir.as_str()));
    let encoded = BASE64.encode(tgz);
    encoded
        .as_bytes()
        .chunks(UPLOAD_CHUNK_B64_LEN)
        .map(|chunk| {
            // Chunks only contain base64 characters, which are ASCII.
            let chunk = String::from_utf8_lossy(chunk);
            format!("printf %s {chunk} | base64 -d >> {payload}")
        })
        .collect()
}

/// Script that verifies the uploaded payload, backs up the current target, then extracts.
/// Prints `backup=<path>` when a backup was made.
pub fn upload_commit_script(commit: &UploadCommit<'_>) -> String {
    let tmp_dir = posix_quote(commit.tmp_dir.as_str());
    let parent = posix_quote(commit.parent);
    let name = posix_quote(&format!("./{}", commit.name));
    let backup_file = posix_quote(&format!("{}.tgz", commit.backup_name));
    let expected_len = commit.expected_len;
    let flags = commit.extract_mode.tar_flags();
    format!(
        r#"T={tmp_dir}; P={parent}; N={name}
[ "$(wc -c < "$T/{PAYLOAD_FILE_NAME}" | tr -d ' ')" = "{expected_len}" ] || {{ echo "size mismatch"; exit {EXIT_SIZE_MISMATCH}; }}
gzip -t "$T/{PAYLOAD_FILE_NAME}" || {{ echo "corrupt payload"; exit {EXIT_CORRUPT_PAYLOAD}; }}
if [ -e "$P/$N" ]; then
  B="{BACKUP_DIR}"
  if [ -L "$HOME/.warp-sync" ] || [ -L "$B" ]; then echo "unsafe backup directory"; exit {EXIT_BACKUP_DIR}; fi
  mkdir -p "$B" && chmod 700 "$B" || {{ echo "cannot create backup directory"; exit {EXIT_BACKUP_DIR}; }}
  tar -czf "$B"/{backup_file} -C "$P" "$N" || {{ echo "backup failed"; exit {EXIT_BACKUP_FAILED}; }}
  echo "backup=$B/"{backup_file}
fi
tar -xzf "$T/{PAYLOAD_FILE_NAME}" -C "$P" {flags} || {{ echo "extract failed"; exit {EXIT_EXTRACT_FAILED}; }}
rm -rf "$T"
"#
    )
}

pub fn cleanup_command(tmp_dir: &RemoteTmpDir) -> String {
    format!("rm -rf {}", posix_quote(tmp_dir.as_str()))
}

pub fn parse_probe_output(output: &str) -> Result<ProbeResult, WarpSyncError> {
    let mut status = None;
    let mut user = None;
    let mut uid = None;
    let mut kind = None;
    let mut size_kib = None;
    let mut tar = None;
    let mut has_base64 = None;
    let mut machine_id = None;

    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "status" => {
                status = match value {
                    "ok" => Some(ProbeStatus::Ok),
                    "not_found" => Some(ProbeStatus::NotFound),
                    "permission_denied" => Some(ProbeStatus::PermissionDenied),
                    _ => return Err(unexpected_output("unknown probe status")),
                }
            }
            "user" => user = Some(value.to_owned()),
            "uid" => uid = value.parse().ok(),
            "kind" => {
                kind = match value {
                    "dir" => Some(RemoteKind::Dir),
                    "file" => Some(RemoteKind::File),
                    _ => None,
                }
            }
            "size_kib" => size_kib = value.parse().ok(),
            "tar" => {
                tar = match value {
                    "gnu" => Some(TarFlavor::Gnu),
                    "other" => Some(TarFlavor::Other),
                    _ => None,
                }
            }
            "base64" => has_base64 = Some(value == "yes"),
            "machine_id" => machine_id = valid_machine_id(value),
            _ => {}
        }
    }

    let status = status.ok_or_else(|| unexpected_output("missing `status`"))?;
    if status == ProbeStatus::NotFound {
        return Ok(ProbeResult {
            status,
            user: String::new(),
            uid: 0,
            kind: RemoteKind::File,
            size_kib: None,
            tar: TarFlavor::Other,
            has_base64: false,
            machine_id: None,
        });
    }
    Ok(ProbeResult {
        status,
        user: user.ok_or_else(|| unexpected_output("missing `user`"))?,
        uid: uid.ok_or_else(|| unexpected_output("missing `uid`"))?,
        kind: kind.ok_or_else(|| unexpected_output("missing `kind`"))?,
        size_kib,
        tar: tar.ok_or_else(|| unexpected_output("missing `tar`"))?,
        has_base64: has_base64.ok_or_else(|| unexpected_output("missing `base64`"))?,
        machine_id,
    })
}

/// Machine ids are hex strings (32 characters on systemd hosts); anything else is ignored rather
/// than trusted.
fn valid_machine_id(value: &str) -> Option<String> {
    let is_valid = (MIN_MACHINE_ID_LEN..=MAX_MACHINE_ID_LEN).contains(&value.len())
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    is_valid.then(|| value.to_owned())
}

/// The last non-empty line of the tail of a failed command's output, which is where tools print
/// the reason.
pub fn remote_failure_message(output: &[u8]) -> String {
    let tail = &output[output.len().saturating_sub(FAILURE_MESSAGE_TAIL_BYTES)..];
    String::from_utf8_lossy(tail)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

fn unexpected_output(message: &str) -> WarpSyncError {
    WarpSyncError::RemoteCommandFailed {
        exit_code: None,
        message: message.to_owned(),
    }
}

#[cfg(test)]
#[path = "remote_script_tests.rs"]
mod tests;
