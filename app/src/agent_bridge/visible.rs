//! `remote.exec.visible`: a command typed into the attached session's own shell, so that it runs
//! as a block the user watches. The request is answered from that block.

use std::time::Duration;

use ::local_control::protocol::{RemoteExecVisibleParams, RemoteExecVisibleResult, RemoteSessionRef};


use super::error::AgentBridgeError;
use super::ops::validate_command;
use super::recent::{CapturedBlock, capture_block, limit_output};
use super::{
    EXEC_DEFAULT_TIMEOUT_SECS, VISIBLE_FAST_POLL_WINDOW, VISIBLE_OUTPUT_MAX_BYTES,
    VISIBLE_POLL_INTERVAL, VISIBLE_SLOW_POLL_INTERVAL,
};
use crate::terminal::model::block::{Block, BlockId};
use crate::terminal::model::blocks::BlockList;
use crate::terminal::model::session::SessionId;
use crate::terminal::model::terminal_model::BlockIndex;

pub(crate) fn validate(params: &RemoteExecVisibleParams) -> Result<(), AgentBridgeError> {
    validate_command(&params.command, params.timeout_secs, params.agent.as_deref())
}

/// How long to wait for the command before answering with what it has printed so far.
pub(crate) fn timeout(params: &RemoteExecVisibleParams) -> Duration {
    Duration::from_secs(params.timeout_secs.unwrap_or(EXEC_DEFAULT_TIMEOUT_SECS).into())
}

/// The pause before the next look at the block, `elapsed` after the command was sent.
pub(crate) fn poll_interval(elapsed: Duration) -> Duration {
    if elapsed < VISIBLE_FAST_POLL_WINDOW {
        VISIBLE_POLL_INTERVAL
    } else {
        VISIBLE_SLOW_POLL_INTERVAL
    }
}

/// The first block at or after `start` in which `command` started in `session`. Matching the
/// command text as well as the position keeps a command the user typed at the same moment from
/// being taken for the agent's.
pub(crate) fn find_command_block<'a>(
    block_list: &'a BlockList,
    start: BlockIndex,
    session: SessionId,
    command: &str,
) -> Option<&'a Block> {
    block_list.blocks().iter().skip(start.0).find(|block| {
        block.session_id() == Some(session)
            && block.started()
            && block.command_to_string().trim() == command.trim()
    })
}

/// Where the wait for a visible command stands after one look at the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Progress {
    /// The command has not shown up in a block yet.
    NotStarted,
    Running,
    Finished(CapturedBlock),
}

/// Follows the block of one visible command across looks at the terminal model.
#[derive(Debug, Clone)]
pub(crate) struct CommandWatch {
    session: SessionId,
    command: String,
    start: BlockIndex,
    block_id: Option<BlockId>,
}

impl CommandWatch {
    /// `start` is the index of the active block when the command was sent: the command runs in
    /// that block or a later one.
    pub(crate) fn new(session: SessionId, command: String, start: BlockIndex) -> Self {
        Self {
            session,
            command,
            start,
            block_id: None,
        }
    }

    pub(crate) fn poll(&mut self, block_list: &BlockList) -> Result<Progress, AgentBridgeError> {
        let Some(block) = self.block(block_list)? else {
            return Ok(Progress::NotStarted);
        };
        if block.is_done() {
            Ok(Progress::Finished(capture_block(block)))
        } else {
            Ok(Progress::Running)
        }
    }

    /// What the command has printed so far, once it has started.
    pub(crate) fn snapshot(&self, block_list: &BlockList) -> Option<CapturedBlock> {
        let id = self.block_id.as_ref()?;
        block_list.block_with_id(id).map(capture_block)
    }

    fn block<'a>(&mut self, block_list: &'a BlockList) -> Result<Option<&'a Block>, AgentBridgeError> {
        if let Some(id) = &self.block_id {
            return block_list.block_with_id(id).map(Some).ok_or_else(|| {
                AgentBridgeError::Executor(
                    "the command's block was removed from the terminal before it finished"
                        .to_owned(),
                )
            });
        }
        let block = find_command_block(block_list, self.start, self.session, &self.command);
        self.block_id = block.map(|block| block.id().clone());
        Ok(block)
    }
}

/// The answer for a command that finished, or that was still running when the wait ended.
pub(crate) fn result(
    session: RemoteSessionRef,
    captured: CapturedBlock,
    still_running: bool,
    alt_screen: bool,
    duration: Duration,
) -> RemoteExecVisibleResult {
    let (output, truncated) =
        limit_output(captured.output, captured.output_rows, VISIBLE_OUTPUT_MAX_BYTES);
    RemoteExecVisibleResult {
        session,
        command: captured.command,
        cwd: captured.cwd,
        exit_code: (!still_running).then_some(captured.exit_code),
        still_running,
        alt_screen,
        duration_ms: duration.as_millis().try_into().unwrap_or(u64::MAX),
        output,
        output_rows: captured.output_rows as u64,
        truncated,
    }
}

#[cfg(test)]
#[path = "visible_tests.rs"]
mod tests;
