use ::local_control::protocol::RemoteExecVisibleParams;

use super::*;
use crate::agent_bridge::{EXEC_MAX_TIMEOUT_SECS, MAX_COMMAND_BYTES};
use crate::terminal::model::TerminalModel;
use crate::terminal::model::ansi::{ClearMode, Handler};

const ATTACHED: u64 = 7;
const OTHER: u64 = 8;

fn session() -> SessionId {
    SessionId::from(ATTACHED)
}

/// A model whose blocks ran in `ATTACHED` unless their command starts with "local".
fn model_with_blocks(commands: &[(&str, &str)]) -> TerminalModel {
    let mut model = TerminalModel::mock(None, None);
    for (command, output) in commands {
        model.simulate_block(*command, *output);
    }
    assign_sessions(&mut model);
    model
}

fn assign_sessions(model: &mut TerminalModel) {
    for block in model.block_list_mut().blocks_mut() {
        let session = if block.command_to_string().starts_with("local") {
            OTHER
        } else {
            ATTACHED
        };
        block.set_session_id(SessionId::from(session));
    }
}

fn params(command: &str, timeout_secs: Option<u32>) -> RemoteExecVisibleParams {
    RemoteExecVisibleParams {
        command: command.to_owned(),
        timeout_secs,
        agent: Some("claude-code".to_owned()),
    }
}

fn block_index_of(block_list: &BlockList, command: &str) -> BlockIndex {
    let position = block_list
        .blocks()
        .iter()
        .position(|block| block.command_to_string() == command)
        .expect("the block exists");
    BlockIndex(position)
}

#[test]
fn validation_matches_remote_exec() {
    assert_eq!(validate(&params("systemctl status nginx", Some(30))), Ok(()));
    for invalid in [
        params("  ", None),
        params("echo\0", None),
        params(&"x".repeat(MAX_COMMAND_BYTES + 1), None),
        params("ls", Some(0)),
        params("ls", Some(EXEC_MAX_TIMEOUT_SECS + 1)),
        RemoteExecVisibleParams {
            agent: Some("bad agent".to_owned()),
            ..params("ls", None)
        },
    ] {
        assert!(
            matches!(validate(&invalid), Err(AgentBridgeError::InvalidParams(_))),
            "{invalid:?}"
        );
    }
}

#[test]
fn timeout_defaults_to_the_exec_default() {
    assert_eq!(
        timeout(&params("ls", None)),
        Duration::from_secs(EXEC_DEFAULT_TIMEOUT_SECS.into())
    );
    assert_eq!(timeout(&params("ls", Some(5))), Duration::from_secs(5));
}

#[test]
fn polling_slows_down_once_the_command_proves_slow() {
    assert_eq!(poll_interval(Duration::ZERO), VISIBLE_POLL_INTERVAL);
    assert_eq!(
        poll_interval(VISIBLE_FAST_POLL_WINDOW - Duration::from_millis(1)),
        VISIBLE_POLL_INTERVAL
    );
    assert_eq!(
        poll_interval(VISIBLE_FAST_POLL_WINDOW),
        VISIBLE_SLOW_POLL_INTERVAL
    );
    assert_eq!(
        poll_interval(Duration::from_secs(600)),
        VISIBLE_SLOW_POLL_INTERVAL
    );
}

#[test]
fn the_command_block_is_found_at_or_after_the_start() {
    let model = model_with_blocks(&[("uptime", "old"), ("ls", "a"), ("uptime", "new")]);
    let block_list = model.block_list();

    let found = find_command_block(block_list, BlockIndex(0), session(), "uptime")
        .expect("an earlier uptime exists");
    assert!(found.output_grid().content_summary(10, 10, false).contains("old"));

    let start = block_index_of(block_list, "ls");
    let found = find_command_block(block_list, start, session(), " uptime ")
        .expect("the later uptime is found despite surrounding spaces");
    assert!(found.output_grid().content_summary(10, 10, false).contains("new"));
}

#[test]
fn blocks_of_other_sessions_and_other_commands_are_not_taken() {
    let model = model_with_blocks(&[("local_uptime", "x"), ("whoami", "root")]);
    let block_list = model.block_list();
    assert!(find_command_block(block_list, BlockIndex(0), session(), "local_uptime").is_none());
    assert!(find_command_block(block_list, BlockIndex(0), session(), "id").is_none());
    assert!(
        find_command_block(block_list, BlockIndex(0), SessionId::from(99), "whoami").is_none()
    );
}

#[test]
fn the_active_prompt_block_is_not_the_command_block() {
    let model = model_with_blocks(&[("ls", "a")]);
    let block_list = model.block_list();
    assert!(find_command_block(block_list, block_list.active_block_index(), session(), "").is_none());
}

#[test]
fn a_watch_reports_a_finished_command() {
    let model = model_with_blocks(&[("ls", "a"), ("hostname", "prod-1")]);
    let start = block_index_of(model.block_list(), "ls");
    let mut watch = CommandWatch::new(session(), "hostname".to_owned(), start);

    let Ok(Progress::Finished(captured)) = watch.poll(model.block_list()) else {
        panic!("the hostname block has finished");
    };
    assert_eq!(captured.command, "hostname");
    assert_eq!(captured.exit_code, 0);
    assert!(captured.output.contains("prod-1"), "{:?}", captured.output);
}

#[test]
fn a_watch_waits_for_the_command_to_start_and_finish() {
    let mut model = model_with_blocks(&[("ls", "a")]);
    let start = model.block_list().active_block_index();
    let mut watch = CommandWatch::new(session(), "tail -f log".to_owned(), start);
    assert_eq!(watch.poll(model.block_list()), Ok(Progress::NotStarted));
    assert!(watch.snapshot(model.block_list()).is_none());

    model.simulate_long_running_block("tail -f log", "waiting");
    assign_sessions(&mut model);
    assert_eq!(watch.poll(model.block_list()), Ok(Progress::Running));
    let snapshot = watch
        .snapshot(model.block_list())
        .expect("a started command has a snapshot");
    assert!(snapshot.output.contains("waiting"), "{:?}", snapshot.output);
}

#[test]
fn a_watch_fails_when_the_block_disappears() {
    let mut model = model_with_blocks(&[("ls", "a"), ("hostname", "prod-1")]);
    let mut watch = CommandWatch::new(session(), "hostname".to_owned(), BlockIndex(0));
    assert!(matches!(
        watch.poll(model.block_list()),
        Ok(Progress::Finished(_))
    ));

    model
        .block_list_mut()
        .clear_screen(ClearMode::ResetAndClear);
    assert!(matches!(
        watch.poll(model.block_list()),
        Err(AgentBridgeError::Executor(_))
    ));
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

fn outcome(
    captured: CapturedBlock,
    still_running: bool,
    alt_screen: bool,
    duration: Duration,
) -> Outcome {
    Outcome {
        captured,
        still_running,
        alt_screen,
        duration,
    }
}

fn session_ref() -> RemoteSessionRef {
    RemoteSessionRef {
        session_id: "12".to_owned(),
        host: "prod-1".to_owned(),
        user: "root".to_owned(),
    }
}

#[test]
fn a_finished_result_carries_the_exit_code() {
    let result = result(
        session_ref(),
        outcome(captured("error: oops".to_owned(), 1), false, false, Duration::from_millis(1500)),
    );
    assert_eq!(result.exit_code, Some(2));
    assert!(!result.still_running);
    assert_eq!(result.duration_ms, 1500);
    assert_eq!(result.output, "error: oops");
    assert_eq!(result.cwd.as_deref(), Some("/srv"));
    assert!(!result.truncated);
}

#[test]
fn a_running_result_has_no_exit_code() {
    let result = result(
        session_ref(),
        outcome(captured("waiting".to_owned(), 1), true, true, Duration::from_secs(5)),
    );
    assert_eq!(result.exit_code, None);
    assert!(result.still_running);
    assert!(result.alt_screen);
}

#[test]
fn a_large_output_is_cut_to_the_visible_limit() {
    let output = format!("START{}END", "x".repeat(VISIBLE_OUTPUT_MAX_BYTES * 2));
    let result = result(
        session_ref(),
        outcome(captured(output, 1), false, false, Duration::ZERO),
    );
    assert!(result.truncated);
    assert!(result.output.starts_with("START"));
    assert!(result.output.ends_with("END"));
    let note_allowance = 64;
    assert!(result.output.len() <= VISIBLE_OUTPUT_MAX_BYTES + note_allowance);
}

fn running_watch() -> (TerminalModel, CommandWatch) {
    let mut model = model_with_blocks(&[("ls", "a")]);
    let start = model.block_list().active_block_index();
    let watch = CommandWatch::new(session(), "tail -f log".to_owned(), start);
    model.simulate_long_running_block("tail -f log", "waiting");
    assign_sessions(&mut model);
    (model, watch)
}

#[test]
fn a_step_keeps_waiting_while_the_command_runs_within_the_timeout() {
    let (model, mut watch) = running_watch();
    let timeout = Duration::from_secs(30);
    assert_eq!(
        step(&mut watch, &model, Duration::from_secs(29), timeout),
        Ok(None)
    );
}

#[test]
fn a_step_answers_with_a_snapshot_once_the_timeout_passes() {
    let (model, mut watch) = running_watch();
    let timeout = Duration::from_secs(30);
    let outcome = step(&mut watch, &model, timeout, timeout)
        .expect("no error")
        .expect("the wait is over");
    assert!(outcome.still_running);
    assert!(!outcome.alt_screen);
    assert_eq!(outcome.duration, timeout);
    assert!(outcome.captured.output.contains("waiting"), "{outcome:?}");
}

#[test]
fn a_step_answers_as_soon_as_the_command_finishes() {
    let model = model_with_blocks(&[("ls", "a"), ("hostname", "prod-1")]);
    let mut watch = CommandWatch::new(session(), "hostname".to_owned(), BlockIndex(0));
    let outcome = step(
        &mut watch,
        &model,
        Duration::from_millis(400),
        Duration::from_secs(30),
    )
    .expect("no error")
    .expect("the command finished");
    assert!(!outcome.still_running);
    assert_eq!(outcome.duration, Duration::from_millis(400));
}

#[test]
fn a_command_that_never_starts_fails_after_the_start_timeout() {
    let model = model_with_blocks(&[("ls", "a")]);
    let start = model.block_list().active_block_index();
    let mut watch = CommandWatch::new(session(), "uptime".to_owned(), start);
    let timeout = Duration::from_secs(120);
    assert_eq!(
        step(&mut watch, &model, Duration::from_secs(9), timeout),
        Ok(None)
    );
    assert!(matches!(
        step(&mut watch, &model, VISIBLE_START_TIMEOUT, timeout),
        Err(AgentBridgeError::Executor(_))
    ));
    let short = Duration::from_secs(3);
    assert!(matches!(
        step(&mut watch, &model, short, short),
        Err(AgentBridgeError::Executor(_))
    ));
}
