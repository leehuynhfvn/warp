//! `warpctrl mcp`: an MCP server on stdio that lets an agent such as Claude Code use the remote
//! sessions the user has allowed agents to control.
mod edit;
mod format;
mod jsonrpc;
mod redact;
mod tools;

use std::io;
use std::time::Duration;

use clap::Args;
use local_control::protocol::{ActionKind, ControlError, ErrorCode};
use serde_json::Value;

use crate::local_control::commands::send_action;
use crate::local_control::{EXIT_SUCCESS, TargetArgs};
use jsonrpc::serve;
use redact::Redactor;
use tools::{ControlTransport, Tools};

/// Serve the remote-session tools to an agent over MCP on stdin and stdout.
///
/// Register it with the agent, for example `claude mcp add --scope user warp-bridge -- <warp>
/// --warpctrl mcp`. The tools only work in sessions the user allowed from the Warp Command
/// Palette: "Agent Bridge: Allow agents to control this session".
#[derive(Debug, Clone, Args)]
pub struct McpArgs {
    /// Use the local Warp instance with this id from `warpctrl instance list` instead of the only
    /// running one.
    #[arg(long = "instance")]
    pub instance: Option<String>,

    /// Show secrets in file contents and command output to the agent instead of hiding them.
    #[arg(long = "no-redact")]
    pub no_redact: bool,
}

pub(super) fn run_mcp(args: McpArgs) -> Result<u8, ControlError> {
    let redactor = if args.no_redact {
        Redactor::disabled()
    } else {
        Redactor::with_default_patterns()
    };
    let transport = LocalControlTransport {
        instance: args.instance,
    };
    let mut tools = Tools::new(transport, redactor);
    match serve(io::stdin().lock(), io::stdout().lock(), &mut tools) {
        Ok(()) => Ok(EXIT_SUCCESS),
        // The agent went away while an answer was being written.
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(EXIT_SUCCESS),
        Err(err) => Err(ControlError::with_details(
            ErrorCode::Internal,
            "the MCP connection failed",
            err.to_string(),
        )),
    }
}

/// Reaches the running Warp app through local control, looking the instance up on every call
/// so that a restarted Warp is found again.
struct LocalControlTransport {
    instance: Option<String>,
}

impl ControlTransport for LocalControlTransport {
    fn call(
        &mut self,
        action: ActionKind,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, ControlError> {
        let target = TargetArgs {
            instance: self.instance.clone(),
            session: session.map(str::to_owned),
            ..TargetArgs::default()
        };
        send_action(&target, action, params, timeout)
    }
}
