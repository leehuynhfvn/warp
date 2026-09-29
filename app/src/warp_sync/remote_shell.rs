use std::sync::Arc;

use async_trait::async_trait;
use warp_completer::completer::CommandExitStatus;
use warpui::r#async::FutureExt as _;

use super::remote_script::remote_failure_message;
use super::{COMMAND_TIMEOUT, WarpSyncError};
use crate::terminal::model::session::{ExecuteCommandOptions, Session, SessionType};
use crate::terminal::shell::ShellType;

/// A shell on the remote host that commands can be run in. Sync runs with whatever privileges
/// that shell has.
#[async_trait]
pub trait RemoteShell: Send + Sync {
    /// Runs `command` and returns its stdout. A non-zero exit is an error carrying the reason
    /// the command printed.
    async fn run(&self, command: &str) -> Result<Vec<u8>, WarpSyncError>;

    /// What the user typed after `ssh` to open this session, when they did.
    fn ssh_host(&self) -> Option<String> {
        None
    }
}

/// The command executor of a Warpified remote session.
pub struct SessionShell {
    session: Arc<Session>,
}

impl SessionShell {
    pub fn new(session: Arc<Session>) -> Result<Self, WarpSyncError> {
        match session.session_type() {
            SessionType::WarpifiedRemote { .. } => {}
            SessionType::Local => return Err(WarpSyncError::NotRemoteSession),
        }
        match session.shell().shell_type() {
            ShellType::PowerShell => return Err(WarpSyncError::UnsupportedShell),
            ShellType::Zsh | ShellType::Bash | ShellType::Fish => {}
        }
        Ok(Self { session })
    }

    pub fn hostname(&self) -> &str {
        self.session.hostname()
    }
}

#[async_trait]
impl RemoteShell for SessionShell {
    fn ssh_host(&self) -> Option<String> {
        let info = self.session.subshell_info().as_ref()?;
        info.ssh_connection_info.as_ref()?.host.clone()
    }

    async fn run(&self, command: &str) -> Result<Vec<u8>, WarpSyncError> {
        let output = self
            .session
            .execute_command(command, None, None, ExecuteCommandOptions::default())
            .with_timeout(COMMAND_TIMEOUT)
            .await
            .map_err(|_| WarpSyncError::Timeout)?
            .map_err(|err| WarpSyncError::Executor(format!("{err:#}")))?;

        match output.status {
            CommandExitStatus::Success => Ok(output.stdout),
            CommandExitStatus::Failure => {
                // The in-band executor reports a failed command's output on stderr; others may
                // leave it on stdout.
                let message_source = if output.stderr.is_empty() {
                    &output.stdout
                } else {
                    &output.stderr
                };
                Err(WarpSyncError::RemoteCommandFailed {
                    exit_code: output.exit_code.map(|code| code.value()),
                    message: remote_failure_message(message_source),
                })
            }
        }
    }
}
