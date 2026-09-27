//! The commands the user ran in an attached session, read from the session's blocks so that an
//! agent can see an error the user just hit without running anything.

use ::local_control::protocol::RemoteCommandBlock;

use super::error::AgentBridgeError;
use super::{RECENT_OUTPUT_DEFAULT_COUNT, RECENT_OUTPUT_MAX_BYTES, RECENT_OUTPUT_MAX_COUNT};
use crate::terminal::model::block::Block;
use crate::terminal::model::blocks::BlockList;
use crate::terminal::model::session::SessionId;

/// Rows kept from each end of a block's output before the byte limit applies. They bound the
/// work done while the terminal model is locked.
const SUMMARY_HEAD_ROWS: usize = 200;
const SUMMARY_TAIL_ROWS: usize = 400;

/// Share of the byte limit given to the start of the output; errors are usually at the end.
const HEAD_BYTES_DIVISOR: usize = 4;

/// A block's contents, copied out of the terminal model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedBlock {
    pub command: String,
    pub exit_code: i32,
    pub cwd: Option<String>,
    pub output: String,
    pub output_rows: usize,
}

/// The number of blocks to return for the requested `count`.
pub(crate) fn block_count(requested: Option<u32>) -> Result<usize, AgentBridgeError> {
    match requested.unwrap_or(RECENT_OUTPUT_DEFAULT_COUNT) {
        count @ 1..=RECENT_OUTPUT_MAX_COUNT => Ok(count as usize),
        count => Err(AgentBridgeError::InvalidParams(format!(
            "count must be between 1 and {RECENT_OUTPUT_MAX_COUNT}, not {count}"
        ))),
    }
}

/// The last `count` finished commands run in `session`, oldest first. Blocks of other sessions
/// in the same pane (the local shell before `ssh`, a shell before `sudo -i`) are left out: the
/// user attached only this one.
pub(crate) fn capture(
    block_list: &BlockList,
    session: SessionId,
    count: usize,
) -> Vec<CapturedBlock> {
    let scope = block_list.transcript_scope();
    let mut blocks = block_list
        .blocks()
        .iter()
        .rev()
        .filter(|block| {
            block.session_id() == Some(session) && block.is_done() && block.can_be_ai_context(scope)
        })
        .filter(|block| !block.command_to_string().trim().is_empty())
        .map(capture_block)
        .take(count)
        .collect::<Vec<_>>();
    blocks.reverse();
    blocks
}

/// Copies a block out of the terminal model, keeping only both ends of a long output.
pub(crate) fn capture_block(block: &Block) -> CapturedBlock {
    let output_grid = block.output_grid();
    CapturedBlock {
        command: block.command_to_string(),
        exit_code: block.exit_code().value(),
        cwd: block.pwd().cloned(),
        output: output_grid.content_summary(SUMMARY_HEAD_ROWS, SUMMARY_TAIL_ROWS, false),
        output_rows: output_grid.len(),
    }
}

pub(crate) fn command_block(block: CapturedBlock) -> RemoteCommandBlock {
    let (output, truncated) =
        limit_output(block.output, block.output_rows, RECENT_OUTPUT_MAX_BYTES);
    RemoteCommandBlock {
        command: block.command,
        exit_code: block.exit_code,
        cwd: block.cwd,
        output,
        output_rows: block.output_rows as u64,
        truncated,
    }
}

/// Cuts a captured output to `max_bytes`, and whether anything of the whole output is missing.
pub(crate) fn limit_output(output: String, output_rows: usize, max_bytes: usize) -> (String, bool) {
    let rows_cut = output_rows > SUMMARY_HEAD_ROWS + SUMMARY_TAIL_ROWS;
    let (output, bytes_cut) = limit_bytes(output, max_bytes);
    (output, rows_cut || bytes_cut)
}

/// Keeps the start and the end of `text` within `max_bytes`, and whether anything was cut.
fn limit_bytes(text: String, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let head_end = floor_char_boundary(&text, max_bytes / HEAD_BYTES_DIVISOR);
    let tail_start = ceil_char_boundary(&text, text.len() - (max_bytes - head_end));
    let omitted = tail_start - head_end;
    let limited = format!(
        "{}\n... ({omitted} bytes omitted) ...\n{}",
        &text[..head_end],
        &text[tail_start..]
    );
    (limited, true)
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    (0..=index)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0)
}

fn ceil_char_boundary(text: &str, index: usize) -> usize {
    (index..=text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(text.len())
}

#[cfg(test)]
#[path = "recent_tests.rs"]
mod tests;
