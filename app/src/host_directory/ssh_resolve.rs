//! What OpenSSH itself makes of an alias, from `ssh -G`. OpenSSH resolves `Include`, `Match` and
//! wildcards, so Warp never keeps its own copy of the connection details.

use std::io::Read as _;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use command::blocking::Command;
use instant::Instant;

use super::model::validate_alias;

const SSH_G_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_OUTPUT_BYTES: u64 = 256 * 1024;

/// The connection details `ssh` would use for an alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) hostname: String,
    pub(crate) user: String,
    pub(crate) port: u16,
    pub(crate) proxy_jump: Option<String>,
    pub(crate) identity_files: Vec<String>,
}

impl Resolved {
    /// `user@hostname`, with the port when it is not the default.
    pub(crate) fn display(&self) -> String {
        match self.port {
            22 => format!("{}@{}", self.user, self.hostname),
            port => format!("{}@{}:{port}", self.user, self.hostname),
        }
    }
}

/// Parses the output of `ssh -G`: one lowercase keyword and its value per line.
pub(crate) fn parse_ssh_g(output: &str) -> Result<Resolved, String> {
    let mut hostname = None;
    let mut user = None;
    let mut port = None;
    let mut proxy_jump = None;
    let mut identity_files = Vec::new();
    for line in output.lines() {
        let Some((keyword, value)) = line.split_once(' ') else {
            continue;
        };
        let value = value.trim();
        match keyword {
            "hostname" => hostname = Some(value.to_owned()),
            "user" => user = Some(value.to_owned()),
            "port" => {
                port = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| format!("ssh reported the port \"{value}\""))?,
                );
            }
            "proxyjump" if !value.eq_ignore_ascii_case("none") => {
                proxy_jump = Some(value.to_owned());
            }
            "identityfile" => identity_files.push(value.to_owned()),
            _ => {}
        }
    }
    match (hostname, user, port) {
        (Some(hostname), Some(user), Some(port)) => Ok(Resolved {
            hostname,
            user,
            port,
            proxy_jump,
            identity_files,
        }),
        _ => Err("ssh did not report a host name, user and port".to_owned()),
    }
}

/// Something that can ask OpenSSH to resolve an alias, so that the code that depends on the answer
/// can be tested without running `ssh`.
pub(crate) trait SshResolver {
    /// Resolves `alias` using only `config` when given, and the user's own configuration otherwise.
    fn resolve(&self, config: Option<&Path>, alias: &str) -> Result<Resolved, String>;
}

/// Runs the `ssh` found on the path.
pub(crate) struct SystemSsh;

impl SshResolver for SystemSsh {
    fn resolve(&self, config: Option<&Path>, alias: &str) -> Result<Resolved, String> {
        validate_alias(alias).map_err(|reason| format!("The alias \"{alias}\" {reason}"))?;
        let mut command = Command::new("ssh");
        command.arg("-G");
        if let Some(config) = config {
            command.arg("-F").arg(config);
        }
        let mut child = command
            .arg("--")
            .arg(alias)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("could not run ssh: {err}"))?;

        let deadline = Instant::now() + SSH_G_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("ssh -G did not answer in time".to_owned());
                }
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                Err(err) => return Err(format!("could not wait for ssh: {err}")),
            }
        };

        let mut stdout = String::new();
        if let Some(pipe) = child.stdout.take() {
            let _ = pipe.take(MAX_OUTPUT_BYTES).read_to_string(&mut stdout);
        }
        if !status.success() {
            let mut stderr = String::new();
            if let Some(pipe) = child.stderr.take() {
                let _ = pipe.take(MAX_OUTPUT_BYTES).read_to_string(&mut stderr);
            }
            let reason = stderr.lines().next().unwrap_or("ssh -G failed");
            return Err(reason.to_owned());
        }
        parse_ssh_g(&stdout)
    }
}

#[cfg(test)]
#[path = "ssh_resolve_tests.rs"]
mod tests;
