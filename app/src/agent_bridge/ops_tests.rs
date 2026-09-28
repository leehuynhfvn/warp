use std::fs;
use std::path::Path;
use std::sync::Mutex;

use command::blocking::Command;
use futures::executor::block_on;

use super::*;

/// Runs every command in a real `sh`, with `HOME` and `TMPDIR` pointing into a scratch directory.
struct ShellRunner {
    home: tempfile::TempDir,
    tmp: tempfile::TempDir,
    calls: Mutex<Vec<(String, Duration)>>,
    /// Zero-based index of the call that reports a failure instead of running.
    fail_call: Option<usize>,
    busy: bool,
}

impl ShellRunner {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            tmp: tempfile::tempdir().unwrap(),
            calls: Mutex::new(Vec::new()),
            fail_call: None,
            busy: false,
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn last_call(&self) -> String {
        self.calls.lock().unwrap().last().unwrap().0.clone()
    }

    fn scratch_is_clean(&self) -> bool {
        fs::read_dir(self.tmp.path()).unwrap().next().is_none()
    }
}

#[async_trait]
impl CommandRunner for ShellRunner {
    async fn run(&self, command: &str, timeout: Duration) -> Result<RawOutput, AgentBridgeError> {
        let index = {
            let mut calls = self.calls.lock().unwrap();
            calls.push((command.to_owned(), timeout));
            calls.len() - 1
        };
        if self.busy {
            return Err(AgentBridgeError::SessionBusy);
        }
        if self.fail_call == Some(index) {
            return Ok(RawOutput {
                stdout: Vec::new(),
                stderr: b"base64: write error: No space left on device\n".to_vec(),
                success: false,
            });
        }
        let mut shell = Command::new("sh");
        shell.args(["-c", command]);
        shell.env("HOME", self.home.path());
        shell.env("TMPDIR", self.tmp.path());
        let output = shell.output().expect("sh is available");
        Ok(RawOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

fn target(audit_dir: Option<&Path>, cwd: Option<&str>) -> Target {
    Target {
        session: RemoteSessionRef {
            session_id: "12".to_owned(),
            host: "prod-1".to_owned(),
            user: "root".to_owned(),
        },
        cwd: cwd.map(str::to_owned),
        request_id: Uuid::from_u128(7),
        audit_dir: audit_dir.map(Path::to_path_buf),
        policy_decision: None,
        agent_id: None,
    }
}

fn exec_params(command: &str) -> RemoteExecParams {
    RemoteExecParams {
        command: command.to_owned(),
        cwd: None,
        timeout_secs: None,
        agent: None,
    }
}

fn read_params(path: &str) -> RemoteFileReadParams {
    RemoteFileReadParams {
        path: path.to_owned(),
        agent: None,
    }
}

fn write_params(
    path: &Path,
    content: &[u8],
    expectation: WriteExpectation,
) -> RemoteFileWriteParams {
    RemoteFileWriteParams {
        path: path.to_str().unwrap().to_owned(),
        content_base64: BASE64.encode(content),
        expectation,
        agent: None,
    }
}

fn all_audit_lines(dir: &Path) -> Vec<Value> {
    fs::read_to_string(dir.join("audit.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The closing records: every request also leaves a "started" record before it runs.
fn audit_lines(dir: &Path) -> Vec<Value> {
    all_audit_lines(dir)
        .into_iter()
        .filter(|record| record["result"] != "started")
        .collect()
}

// ---- exec -----------------------------------------------------------------------------------

#[test]
fn invalid_exec_params_are_refused_before_anything_runs() {
    let too_long = "x".repeat(MAX_COMMAND_BYTES + 1);
    let cases = [
        exec_params(""),
        exec_params("   "),
        exec_params("echo\0hi"),
        exec_params(&too_long),
        RemoteExecParams {
            timeout_secs: Some(0),
            ..exec_params("ls")
        },
        RemoteExecParams {
            timeout_secs: Some(EXEC_MAX_TIMEOUT_SECS + 1),
            ..exec_params("ls")
        },
        RemoteExecParams {
            cwd: Some("relative".to_owned()),
            ..exec_params("ls")
        },
        RemoteExecParams {
            cwd: Some("/a\nb".to_owned()),
            ..exec_params("ls")
        },
        RemoteExecParams {
            agent: Some("not a name!".to_owned()),
            ..exec_params("ls")
        },
        RemoteExecParams {
            agent: Some("a".repeat(MAX_AGENT_NAME_BYTES + 1)),
            ..exec_params("ls")
        },
    ];
    for params in cases {
        let runner = ShellRunner::new();
        let result = block_on(exec(&runner, &target(None, None), params.clone()));
        assert!(
            matches!(result, Err(AgentBridgeError::InvalidParams(_))),
            "{params:?} gave {result:?}"
        );
        assert_eq!(runner.call_count(), 0, "{params:?} ran something");
    }
}

#[test]
fn exec_reports_output_exit_code_and_the_session() {
    let runner = ShellRunner::new();
    let value = block_on(exec(
        &runner,
        &target(None, Some("/tmp")),
        exec_params("echo hi; echo oops >&2; exit 2"),
    ))
    .unwrap();

    let result: RemoteExecResult = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(result.exit_code, 2);
    assert!(!result.timed_out);
    assert_eq!(result.stdout.text, "hi\n");
    assert_eq!(result.stderr.text, "oops\n");
    assert_eq!(result.cwd.as_deref(), Some("/tmp"));
    assert_eq!(value["host"], "prod-1");
    assert_eq!(value["user"], "root");
    assert_eq!(value["session_id"], "12");
    assert!(runner.scratch_is_clean());
}

#[test]
fn an_explicit_cwd_wins_over_the_session_directory() {
    let runner = ShellRunner::new();
    let params = RemoteExecParams {
        cwd: Some("/".to_owned()),
        ..exec_params("pwd")
    };
    let value = block_on(exec(&runner, &target(None, Some("/tmp")), params)).unwrap();
    let result: RemoteExecResult = serde_json::from_value(value).unwrap();
    assert_eq!(result.stdout.text, "/\n");
}

#[test]
fn a_session_directory_the_bridge_would_not_accept_is_ignored() {
    let runner = ShellRunner::new();
    let value = block_on(exec(
        &runner,
        &target(None, Some("relative")),
        exec_params("true"),
    ))
    .unwrap();
    let result: RemoteExecResult = serde_json::from_value(value).unwrap();
    assert_eq!(result.cwd, None);
}

#[test]
fn the_runner_waits_for_the_command_timeout_plus_a_grace_period() {
    let runner = ShellRunner::new();
    block_on(exec(&runner, &target(None, None), exec_params("true"))).unwrap();
    let params = RemoteExecParams {
        timeout_secs: Some(30),
        ..exec_params("true")
    };
    block_on(exec(&runner, &target(None, None), params)).unwrap();

    let waits: Vec<Duration> = runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|call| call.1)
        .collect();
    assert_eq!(
        waits,
        [
            Duration::from_secs(EXEC_DEFAULT_TIMEOUT_SECS.into()) + EXEC_TIMEOUT_GRACE,
            Duration::from_secs(30) + EXEC_TIMEOUT_GRACE,
        ]
    );
}

#[test]
fn a_busy_session_is_reported_and_audited() {
    let audit = tempfile::tempdir().unwrap();
    let mut runner = ShellRunner::new();
    runner.busy = true;
    let result = block_on(exec(
        &runner,
        &target(Some(audit.path()), None),
        exec_params("ls"),
    ));
    assert_eq!(result.unwrap_err(), AgentBridgeError::SessionBusy);

    let records = audit_lines(audit.path());
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["result"], "error");
    assert_eq!(records[0]["error_code"], "session_busy");
    assert_eq!(records[0]["command"], "ls");
}

#[test]
fn a_successful_exec_is_audited_without_its_output() {
    let audit = tempfile::tempdir().unwrap();
    let runner = ShellRunner::new();
    let params = RemoteExecParams {
        agent: Some("claude-code".to_owned()),
        ..exec_params("echo secret-output; exit 3")
    };
    block_on(exec(
        &runner,
        &target(Some(audit.path()), Some("/tmp")),
        params,
    ))
    .unwrap();

    let records = audit_lines(audit.path());
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record["action"], "remote.exec");
    assert_eq!(record["agent"], "claude-code");
    assert_eq!(record["request_id"], Uuid::from_u128(7).to_string());
    assert_eq!(record["session_id"], "12");
    assert_eq!(record["host"], "prod-1");
    assert_eq!(record["user"], "root");
    assert_eq!(record["cwd"], "/tmp");
    assert_eq!(record["command"], "echo secret-output; exit 3");
    assert_eq!(record["exit_code"], 3);
    assert_eq!(record["result"], "ok");
    assert!(!record.to_string().contains("secret-output\\n"));
}

#[test]
fn a_refused_request_is_audited_without_the_invalid_agent_name() {
    let audit = tempfile::tempdir().unwrap();
    let runner = ShellRunner::new();
    let params = RemoteExecParams {
        agent: Some("bad name".to_owned()),
        ..exec_params("ls")
    };
    block_on(exec(&runner, &target(Some(audit.path()), None), params)).unwrap_err();

    let records = audit_lines(audit.path());
    assert_eq!(records[0]["error_code"], "invalid_params");
    assert!(records[0].get("agent").is_none());
}

// ---- read -----------------------------------------------------------------------------------

#[test]
fn read_returns_the_content_checksum_and_size() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("app.conf"), "listen 80;\n").unwrap();
    let cwd = dir.path().to_str().unwrap();

    let value = block_on(read_file(
        &runner,
        &target(None, Some(cwd)),
        read_params("app.conf"),
    ))
    .unwrap();

    let RemoteFileReadResult::Ok {
        path,
        size,
        sha256,
        content_base64,
        session,
    } = serde_json::from_value(value.clone()).unwrap()
    else {
        panic!("expected content, got {value}");
    };
    assert_eq!(path, format!("{cwd}/app.conf"));
    assert_eq!(size, 11);
    assert_eq!(sha256, sha256_hex(b"listen 80;\n"));
    assert_eq!(BASE64.decode(content_base64).unwrap(), b"listen 80;\n");
    assert_eq!(session.host, "prod-1");
    assert_eq!(value["status"], "ok");
}

#[test]
fn a_missing_file_is_a_result_not_an_error() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nope");
    let value = block_on(read_file(
        &runner,
        &target(None, None),
        read_params(path.to_str().unwrap()),
    ))
    .unwrap();
    assert_eq!(value["status"], "not_found");
    assert_eq!(value["host"], "prod-1");
}

#[test]
fn an_oversized_file_is_refused() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big");
    fs::write(&path, vec![b'x'; READ_MAX_FILE_BYTES + 1]).unwrap();
    let result = block_on(read_file(
        &runner,
        &target(None, None),
        read_params(path.to_str().unwrap()),
    ));
    assert!(
        matches!(result, Err(AgentBridgeError::RemoteFailed(_))),
        "{result:?}"
    );
}

#[test]
fn unsafe_read_paths_are_refused_before_anything_runs() {
    for path in ["", "~/.bashrc", "/etc/../shadow", "relative"] {
        let runner = ShellRunner::new();
        let result = block_on(read_file(&runner, &target(None, None), read_params(path)));
        assert!(
            matches!(result, Err(AgentBridgeError::InvalidParams(_))),
            "{path:?}"
        );
        assert_eq!(runner.call_count(), 0);
    }
}

#[test]
fn a_read_is_audited_with_its_size() {
    let audit = tempfile::tempdir().unwrap();
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");
    fs::write(&path, "12345").unwrap();
    block_on(read_file(
        &runner,
        &target(Some(audit.path()), None),
        read_params(path.to_str().unwrap()),
    ))
    .unwrap();

    let records = audit_lines(audit.path());
    assert_eq!(records[0]["action"], "remote.file.read");
    assert_eq!(records[0]["bytes"], 5);
    assert_eq!(records[0]["path"], path.to_str().unwrap());
}

// ---- write ----------------------------------------------------------------------------------

fn must_match(content: &[u8]) -> WriteExpectation {
    WriteExpectation::MustMatch {
        sha256: sha256_hex(content),
    }
}

fn write_result(value: Value) -> RemoteFileWriteResult {
    serde_json::from_value(value).unwrap()
}

#[test]
fn a_new_file_is_created_and_the_scratch_directory_is_removed() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.conf");

    let value = block_on(write_file(
        &runner,
        &target(None, None),
        write_params(&path, b"fresh\n", WriteExpectation::MustNotExist),
    ))
    .unwrap();

    let result = write_result(value);
    assert!(result.created);
    assert_eq!(result.backup_path, None);
    assert_eq!(result.bytes, 6);
    assert_eq!(result.sha256, sha256_hex(b"fresh\n"));
    assert_eq!(fs::read(&path).unwrap(), b"fresh\n");
    assert!(runner.scratch_is_clean());
}

#[test]
fn an_empty_file_can_be_written() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty");
    let value = block_on(write_file(
        &runner,
        &target(None, None),
        write_params(&path, b"", WriteExpectation::MustNotExist),
    ))
    .unwrap();
    assert!(write_result(value).created);
    assert_eq!(fs::read(&path).unwrap(), b"");
}

#[test]
fn a_large_file_is_uploaded_in_several_chunks_and_arrives_intact() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big");
    let content: Vec<u8> = (0..200_000u32).map(|n| (n % 251) as u8).collect();

    block_on(write_file(
        &runner,
        &target(None, None),
        write_params(&path, &content, WriteExpectation::MustNotExist),
    ))
    .unwrap();

    assert_eq!(fs::read(&path).unwrap(), content);
    assert!(runner.call_count() > 4);
}

#[test]
fn an_edit_with_the_current_checksum_overwrites_and_backs_up() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.conf");
    fs::write(&path, "old\n").unwrap();

    let value = block_on(write_file(
        &runner,
        &target(None, None),
        write_params(&path, b"new\n", must_match(b"old\n")),
    ))
    .unwrap();

    let result = write_result(value);
    assert!(!result.created);
    assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
    let backup = result.backup_path.expect("an overwrite is backed up");
    assert_eq!(fs::read_to_string(backup).unwrap(), "old\n");
    assert!(runner.scratch_is_clean());
}

#[test]
fn a_stale_checksum_is_a_conflict_that_changes_nothing() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.conf");
    fs::write(&path, "someone else edited this\n").unwrap();

    let result = block_on(write_file(
        &runner,
        &target(None, None),
        write_params(&path, b"new\n", must_match(b"what I read\n")),
    ));

    assert!(
        matches!(result, Err(AgentBridgeError::Conflict(_))),
        "{result:?}"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "someone else edited this\n"
    );
    assert!(runner.scratch_is_clean());
}

#[test]
fn creating_over_an_existing_file_is_a_conflict() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("there");
    fs::write(&path, "keep").unwrap();
    let result = block_on(write_file(
        &runner,
        &target(None, None),
        write_params(&path, b"x", WriteExpectation::MustNotExist),
    ));
    assert!(matches!(result, Err(AgentBridgeError::Conflict(_))));
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep");
}

#[test]
fn a_failed_upload_step_removes_the_scratch_directory_and_reports_why() {
    let mut runner = ShellRunner::new();
    runner.fail_call = Some(1);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");

    let result = block_on(write_file(
        &runner,
        &target(None, None),
        write_params(&path, b"content", WriteExpectation::MustNotExist),
    ));

    let error = result.unwrap_err();
    assert!(error.to_string().contains("No space left"), "{error}");
    assert!(runner.last_call().starts_with("rm -rf "));
    assert!(runner.scratch_is_clean());
    assert!(!path.exists());
}

#[test]
fn invalid_write_params_are_refused_before_anything_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");
    let good = write_params(&path, b"x", WriteExpectation::MustNotExist);
    let cases = [
        RemoteFileWriteParams {
            content_base64: "!!!not base64".to_owned(),
            ..good.clone()
        },
        RemoteFileWriteParams {
            content_base64: BASE64.encode(vec![b'x'; WRITE_MAX_BYTES + 1]),
            ..good.clone()
        },
        RemoteFileWriteParams {
            expectation: WriteExpectation::MustMatch {
                sha256: "ABC".to_owned(),
            },
            ..good.clone()
        },
        RemoteFileWriteParams {
            expectation: WriteExpectation::MustMatch {
                sha256: "A".repeat(SHA256_HEX_LEN),
            },
            ..good.clone()
        },
        RemoteFileWriteParams {
            path: "~/.bashrc".to_owned(),
            ..good.clone()
        },
        RemoteFileWriteParams {
            agent: Some("bad agent".to_owned()),
            ..good
        },
    ];
    for params in cases {
        let runner = ShellRunner::new();
        let result = block_on(write_file(&runner, &target(None, None), params));
        assert!(
            matches!(result, Err(AgentBridgeError::InvalidParams(_))),
            "{result:?}"
        );
        assert_eq!(runner.call_count(), 0);
    }
}

#[test]
fn what_was_written_can_be_read_back_with_the_same_checksum() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("round.trip");
    let content = "line 1\nünïcode ✓\n".as_bytes();
    let written = write_result(
        block_on(write_file(
            &runner,
            &target(None, None),
            write_params(&path, content, WriteExpectation::MustNotExist),
        ))
        .unwrap(),
    );

    let value = block_on(read_file(
        &runner,
        &target(None, None),
        read_params(path.to_str().unwrap()),
    ))
    .unwrap();
    let RemoteFileReadResult::Ok {
        sha256,
        content_base64,
        ..
    } = serde_json::from_value(value).unwrap()
    else {
        panic!("expected content");
    };
    assert_eq!(sha256, written.sha256);
    assert_eq!(BASE64.decode(content_base64).unwrap(), content);
}

#[test]
fn a_write_is_audited_with_its_size_and_never_its_content() {
    let audit = tempfile::tempdir().unwrap();
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");
    block_on(write_file(
        &runner,
        &target(Some(audit.path()), None),
        write_params(&path, b"password=hunter2", WriteExpectation::MustNotExist),
    ))
    .unwrap();

    let records = audit_lines(audit.path());
    assert_eq!(records[0]["action"], "remote.file.write");
    assert_eq!(records[0]["bytes"], 16);
    assert!(!records[0].to_string().contains("hunter2"));
}

#[test]
fn a_request_that_cannot_be_audited_is_not_run() {
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let audit_dir = blocker.path().join("agent-bridge");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");
    fs::write(&path, "x").unwrap();

    let runner = ShellRunner::new();
    let target = target(Some(&audit_dir), None);
    let results = [
        block_on(exec(&runner, &target, exec_params("touch ran"))),
        block_on(read_file(
            &runner,
            &target,
            read_params(path.to_str().unwrap()),
        )),
        block_on(write_file(
            &runner,
            &target,
            write_params(&dir.path().join("g"), b"y", WriteExpectation::MustNotExist),
        )),
    ];

    for result in results {
        assert!(
            matches!(&result, Err(AgentBridgeError::Io(message)) if message.contains("audit log")),
            "{result:?}"
        );
    }
    assert_eq!(runner.call_count(), 0);
}

#[test]
fn every_request_leaves_a_start_record_before_its_closing_record() {
    let audit = tempfile::tempdir().unwrap();
    let runner = ShellRunner::new();
    block_on(exec(
        &runner,
        &target(Some(audit.path()), None),
        exec_params("true"),
    ))
    .unwrap();

    let records = all_audit_lines(audit.path());
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["result"], "started");
    assert_eq!(records[0]["command"], "true");
    assert_eq!(records[1]["result"], "ok");
    assert_eq!(records[0]["request_id"], records[1]["request_id"]);
}

#[test]
fn a_conflict_does_not_send_a_needless_cleanup_command() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("there");
    fs::write(&path, "keep").unwrap();
    block_on(write_file(
        &runner,
        &target(None, None),
        write_params(&path, b"x", WriteExpectation::MustNotExist),
    ))
    .unwrap_err();
    assert!(!runner.last_call().starts_with("rm -rf "));
    assert!(runner.scratch_is_clean());
}

#[test]
fn a_commit_that_cannot_report_still_removes_the_upload_directory() {
    let mut runner = ShellRunner::new();
    // begin, one chunk, then the commit script itself fails to run
    runner.fail_call = Some(2);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f");
    block_on(write_file(
        &runner,
        &target(None, None),
        write_params(&path, b"x", WriteExpectation::MustNotExist),
    ))
    .unwrap_err();
    assert!(runner.last_call().starts_with("rm -rf "));
    assert!(runner.scratch_is_clean());
}

#[test]
fn a_write_whose_encoded_content_is_far_too_long_is_refused_before_decoding() {
    let runner = ShellRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let params = RemoteFileWriteParams {
        content_base64: "A".repeat(WRITE_MAX_BASE64_LEN + 1),
        ..write_params(&dir.path().join("f"), b"", WriteExpectation::MustNotExist)
    };
    let result = block_on(write_file(&runner, &target(None, None), params));
    assert!(matches!(result, Err(AgentBridgeError::InvalidParams(_))));
    assert_eq!(runner.call_count(), 0);
}
