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
pub(in crate::local_control) use format::render_hosts;
use jsonrpc::serve;
use local_control::protocol::{ActionKind, AgentToken, ControlError, ErrorCode};
use redact::Redactor;
use serde_json::Value;
use tools::{ControlTransport, Tools};

use crate::local_control::commands::send_action_with_token;
use crate::local_control::{EXIT_SUCCESS, TargetArgs};

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

    /// Never present a pairing token: every request is from an unpaired client (mục 3.11 of the
    /// O2 agent-ops policy plan).
    #[arg(long = "no-pair")]
    pub no_pair: bool,
}

pub(super) fn run_mcp(args: McpArgs) -> Result<u8, ControlError> {
    let redactor = if args.no_redact {
        Redactor::disabled()
    } else {
        Redactor::with_default_patterns()
    };
    let transport = LocalControlTransport {
        instance: args.instance,
        agent_token: None,
    };
    let mut tools = Tools::new(transport, redactor, args.no_pair);
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
/// so that a restarted Warp is found again. `agent_token`, once pairing sets it, rides along on
/// every later `call` (mục 3.11 of the O2 agent-ops policy plan).
struct LocalControlTransport {
    instance: Option<String>,
    agent_token: Option<AgentToken>,
}

impl LocalControlTransport {
    fn target(&self, session: Option<&str>) -> TargetArgs {
        TargetArgs {
            instance: self.instance.clone(),
            session: session.map(str::to_owned),
            ..TargetArgs::default()
        }
    }
}

impl ControlTransport for LocalControlTransport {
    fn call(
        &mut self,
        action: ActionKind,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, ControlError> {
        let target = self.target(session);
        send_action_with_token(&target, action, params, timeout, self.agent_token.as_ref())
    }

    fn pair(
        &mut self,
        name: &str,
        token: &AgentToken,
        timeout: Duration,
    ) -> Result<Value, ControlError> {
        let target = self.target(None);
        let params = local_control::protocol::AgentPairParams {
            name: name.to_owned(),
        };
        let result =
            send_action_with_token(&target, ActionKind::AgentPair, params, timeout, Some(token));
        // Sent on every later call regardless of outcome: an agent.pair that failed or was denied
        // still leaves the token attached, so Warp simply sees it as unrecognized (mục 3.11) —
        // and so it will be tried again, and can succeed, the next time this token is paired.
        self.agent_token = Some(token.clone());
        result
    }
}
