use super::*;
use crate::terminal::model::TerminalModel;

const ATTACHED: u64 = 7;
const OTHER: u64 = 8;

/// A model whose blocks ran in `ATTACHED` unless their command starts with "local".
fn model_with_blocks(commands: &[(&str, &str)]) -> TerminalModel {
    let mut model = TerminalModel::mock(None, None);
    for (command, output) in commands {
        model.simulate_block(*command, *output);
    }
    for block in model.block_list_mut().blocks_mut() {
        let session = if block.command_to_string().starts_with("local") {
            OTHER
        } else {
            ATTACHED
        };
        block.set_session_id(SessionId::from(session));
    }
    model
}

fn commands(blocks: &[CapturedBlock]) -> Vec<&str> {
    blocks.iter().map(|block| block.command.as_str()).collect()
}

#[test]
fn count_defaults_to_three_and_must_be_between_one_and_ten() {
    assert_eq!(block_count(None), Ok(3));
    assert_eq!(block_count(Some(1)), Ok(1));
    assert_eq!(block_count(Some(10)), Ok(10));
    for count in [0, 11, u32::MAX] {
        assert!(
            matches!(
                block_count(Some(count)),
                Err(AgentBridgeError::InvalidParams(_))
            ),
            "{count}"
        );
    }
}

#[test]
fn capture_returns_the_latest_blocks_of_the_session_oldest_first() {
    let model = model_with_blocks(&[
        ("first", "1"),
        ("local_ssh", "connecting"),
        ("second", "2"),
        ("third", "3"),
    ]);

    let blocks = capture(model.block_list(), SessionId::from(ATTACHED), 2);
    assert_eq!(commands(&blocks), ["second", "third"]);

    let blocks = capture(model.block_list(), SessionId::from(ATTACHED), 10);
    assert_eq!(commands(&blocks), ["first", "second", "third"]);
}

#[test]
fn capture_copies_output_exit_code_and_row_count() {
    let model = model_with_blocks(&[("cat log", "one\r\ntwo")]);

    let blocks = capture(model.block_list(), SessionId::from(ATTACHED), 1);
    let [block] = blocks.as_slice() else {
        panic!("expected one block, got {blocks:?}");
    };
    assert_eq!(block.exit_code, 0);
    assert!(block.output.contains("one"), "{:?}", block.output);
    assert!(block.output.contains("two"), "{:?}", block.output);
    assert_eq!(block.output_rows, 2);
}

#[test]
fn capture_skips_a_command_that_is_still_running() {
    let mut model = model_with_blocks(&[("done", "ok")]);
    model.simulate_long_running_block("tail -f log", "waiting");
    let running = model.block_list().active_block_index();
    model
        .block_list_mut()
        .block_at_mut(running)
        .expect("the running block exists")
        .set_session_id(SessionId::from(ATTACHED));

    let blocks = capture(model.block_list(), SessionId::from(ATTACHED), 5);
    assert_eq!(commands(&blocks), ["done"]);
}

#[test]
fn capture_returns_nothing_for_a_session_without_blocks() {
    let model = model_with_blocks(&[("first", "1")]);
    assert!(capture(model.block_list(), SessionId::from(99), 3).is_empty());
}

fn captured(output: String, output_rows: usize) -> CapturedBlock {
    CapturedBlock {
        command: "make".to_owned(),
        exit_code: 2,
        cwd: Some("/srv".to_owned()),
        output,
        output_rows,
    }
}

#[test]
fn a_small_block_is_returned_whole() {
    let block = command_block(captured("error: oops".to_owned(), 1));
    assert_eq!(block.output, "error: oops");
    assert_eq!(block.output_rows, 1);
    assert_eq!(block.exit_code, 2);
    assert_eq!(block.cwd.as_deref(), Some("/srv"));
    assert!(!block.truncated);
}

#[test]
fn a_block_longer_than_the_row_summary_is_marked_truncated() {
    let block = command_block(captured("head\n...\ntail".to_owned(), 5000));
    assert_eq!(block.output_rows, 5000);
    assert!(block.truncated);
}

#[test]
fn a_large_output_keeps_its_start_and_end_within_the_limit() {
    let output = format!("START{}END", "x".repeat(RECENT_OUTPUT_MAX_BYTES * 2));
    let block = command_block(captured(output, 1));

    assert!(block.truncated);
    assert!(block.output.starts_with("START"));
    assert!(block.output.ends_with("END"));
    assert!(block.output.contains("bytes omitted"));
    let note_allowance = 64;
    assert!(block.output.len() <= RECENT_OUTPUT_MAX_BYTES + note_allowance);
}

#[test]
fn cutting_never_splits_a_multibyte_character() {
    let text = "é".repeat(100);
    for max_bytes in [7, 8, 9, 51] {
        let (limited, cut) = limit_bytes(text.clone(), max_bytes);
        assert!(cut);
        assert!(limited.contains("bytes omitted"), "{limited}");
    }
}
