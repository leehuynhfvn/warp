use chrono::TimeZone as _;
use regex::Regex;

use super::*;
use crate::warp_sync::remote_script::{validate_tmp_dir, wrap_for_any_shell};

const NONCE: &str = "0123456789abcdef";

fn frame(line: &str) -> String {
    format!("{} {line}\n", marker(NONCE))
}

/// What `exec_script` prints for a command with the given results.
fn exec_report(out: &str, err: &str, rc: &str) -> String {
    let m = marker(NONCE);
    format!(
        "{m} stdout {}\n{out}\n{m} end\n{m} stderr {}\n{err}\n{m} end\n{m} rc {rc}\n",
        out.len(),
        err.len()
    )
}

fn parse_exec(output: &str) -> Result<ExecOutput, AgentBridgeError> {
    parse_exec_output(NONCE, output.as_bytes(), b"")
}

fn parse_read(output: &str) -> Result<ReadOutcome, AgentBridgeError> {
    parse_read_output(NONCE, output.as_bytes(), b"", 1024)
}

fn parse_write(output: &str) -> Result<WriteOutcome, AgentBridgeError> {
    parse_write_output(NONCE, output.as_bytes(), b"")
}

fn tmp_dir() -> RemoteTmpDir {
    validate_tmp_dir("/tmp/warp-sync.abc123").unwrap()
}

// ---- builders -------------------------------------------------------------------------------

#[test]
fn nonces_are_hex_and_differ_between_requests() {
    let first = new_nonce();
    assert_eq!(first.len(), NONCE_LEN);
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(first, new_nonce());
}

#[test]
fn exec_script_only_contains_the_command_and_cwd_quoted() {
    for hostile in ["; rm -rf / #", "$(id)", "`id`", "'; reboot; '"] {
        let script = exec_script(NONCE, hostile, Some(hostile), 30);
        let quoted = posix_quote(hostile);
        assert!(script.contains(&quoted));
        assert!(
            !script.replace(&quoted, "").contains(hostile),
            "{hostile:?} appears outside its quotes"
        );
    }
}

#[test]
fn exec_script_without_a_cwd_does_not_change_directory() {
    assert!(!exec_script(NONCE, "ls", None, 30).contains("cd '"));
}

#[test]
fn a_wrapped_script_is_a_single_line_for_any_shell() {
    let wrapped = wrap_for_any_shell(&exec_script(NONCE, "echo 'hi'", Some("/tmp"), 30));
    let shape = Regex::new(r"^printf %s [A-Za-z0-9+/=]+ \| base64 -d \| sh$").unwrap();
    assert!(shape.is_match(&wrapped), "{wrapped}");
}

#[test]
fn read_and_write_scripts_only_contain_paths_quoted() {
    let hostile = "/etc/x'; reboot; '";
    let quoted = posix_quote(hostile);

    let read = read_script(NONCE, hostile, 1024);
    assert!(!read.replace(&quoted, "").contains("reboot"));

    let write = write_commit_script(
        NONCE,
        &tmp_dir(),
        hostile,
        &WriteExpectation::MustMatch {
            sha256: "x'; reboot; '".to_owned(),
        },
        3,
        "b'; reboot; '",
    );
    let without_quoted_values = write
        .replace(&quoted, "")
        .replace(&posix_quote("x'; reboot; '"), "")
        .replace(&posix_quote("b'; reboot; '"), "");
    assert!(!without_quoted_values.contains("reboot"));
}

#[test]
fn backup_names_are_shell_safe_and_carry_the_time() {
    let now = Utc.with_ymd_and_hms(2026, 9, 25, 13, 4, 5).unwrap();
    let name = backup_name("/etc/nginx/sites available/my site.conf", now);
    let shape = Regex::new(r"^my_site\.conf\.20260925T130405\.[0-9a-f]{8}$").unwrap();
    assert!(shape.is_match(&name), "{name}");

    let odd = backup_name("/etc/$(reboot)`x`", now);
    assert!(
        odd.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    );
    assert!(backup_name("/", now).starts_with("file."));
    assert_ne!(backup_name("/etc/a", now), backup_name("/etc/a", now));
}

// ---- exec parser ----------------------------------------------------------------------------

#[test]
fn exec_output_with_both_streams_parses() {
    let parsed = parse_exec(&exec_report("out\nline 2\n", "warning", "3 1")).unwrap();
    assert_eq!(parsed.stdout.text, "out\nline 2\n");
    assert_eq!(parsed.stdout.total_bytes, 11);
    assert!(!parsed.stdout.truncated);
    assert_eq!(parsed.stderr.text, "warning");
    assert_eq!(parsed.exit_code, 3);
    assert!(!parsed.timed_out);
}

#[test]
fn empty_sections_parse_as_empty_text() {
    let parsed = parse_exec(&exec_report("", "", "0 1")).unwrap();
    assert_eq!(parsed.stdout.text, "");
    assert_eq!(parsed.stdout.total_bytes, 0);
    assert_eq!(parsed.stderr.text, "");
    assert_eq!(parsed.exit_code, 0);
}

#[test]
fn a_missing_final_newline_after_the_status_is_fine() {
    let output = exec_report("a", "b", "0 1");
    assert!(parse_exec(output.trim_end()).is_ok());
}

#[test]
fn a_timeout_needs_both_the_timeout_tool_and_its_exit_status() {
    assert!(parse_exec(&exec_report("", "", "124 1")).unwrap().timed_out);
    assert!(!parse_exec(&exec_report("", "", "124 0")).unwrap().timed_out);
    assert!(!parse_exec(&exec_report("", "", "1 1")).unwrap().timed_out);
}

#[test]
fn a_cut_section_keeps_head_and_tail_and_says_how_much_was_omitted() {
    let m = marker(NONCE);
    let total = EXEC_STDOUT_MAX_BYTES * 3;
    let output = format!(
        "{m} stdout {total}\nHEAD\n{m} cut\nTAIL\n{m} end\n{m} stderr 0\n\n{m} end\n{m} rc 0 1\n"
    );
    let parsed = parse_exec(&output).unwrap();
    let omitted = total - EXEC_STDOUT_MAX_BYTES;
    assert_eq!(
        parsed.stdout.text,
        format!("HEAD\n… [{omitted} bytes omitted] …\nTAIL")
    );
    assert!(parsed.stdout.truncated);
    assert_eq!(parsed.stdout.total_bytes, total as u64);
    assert!(!parsed.stderr.truncated);
}

#[test]
fn output_that_is_not_utf8_is_decoded_lossily_without_losing_the_frame() {
    let m = marker(NONCE);
    let mut report = format!("{m} stdout 3\n").into_bytes();
    report.extend_from_slice(b"a\xffb");
    report
        .extend_from_slice(format!("\n{m} end\n{m} stderr 0\n\n{m} end\n{m} rc 0 1\n").as_bytes());

    let parsed = parse_exec_output(NONCE, &report, b"").unwrap();

    assert_eq!(parsed.stdout.text, "a\u{fffd}b");
    assert_eq!(parsed.exit_code, 0);
}

#[test]
fn noise_before_the_frame_is_ignored() {
    let output = format!("$ ls\r\nsome prompt\n{}", exec_report("ok", "", "0 1"));
    assert_eq!(parse_exec(&output).unwrap().stdout.text, "ok");
}

#[test]
fn the_frame_is_found_on_stderr_when_stdout_has_none() {
    let report = exec_report("ok", "", "0 1");
    let parsed = parse_exec_output(NONCE, b"nothing here", report.as_bytes()).unwrap();
    assert_eq!(parsed.stdout.text, "ok");
}

#[test]
fn a_report_without_an_exit_status_is_an_error() {
    let output = exec_report("ok", "", "0 1");
    let without_rc = output
        .rsplit_once(&format!("{} rc", marker(NONCE)))
        .unwrap()
        .0;
    assert!(matches!(
        parse_exec(without_rc),
        Err(AgentBridgeError::UnexpectedOutput(_))
    ));
}

#[test]
fn a_frame_with_another_nonce_is_not_accepted() {
    let other = exec_report("ok", "", "0 1").replace(NONCE, "ffffffffffffffff");
    assert!(matches!(
        parse_exec(&other),
        Err(AgentBridgeError::UnexpectedOutput(_))
    ));
}

#[test]
fn fatal_reports_become_specific_errors() {
    assert_eq!(
        parse_exec(&frame("fatal cwd")).unwrap_err(),
        AgentBridgeError::InvalidParams("cwd does not exist or is not accessible".to_owned())
    );
    assert!(matches!(
        parse_exec(&frame("fatal mktemp")),
        Err(AgentBridgeError::RemoteFailed(_))
    ));
}

#[test]
fn output_without_a_frame_quotes_its_tail() {
    let long = format!("{}bash: sh: command not found", "x".repeat(500));
    let error = parse_exec_output(NONCE, long.as_bytes(), b"").unwrap_err();
    let AgentBridgeError::UnexpectedOutput(tail) = error else {
        panic!("expected UnexpectedOutput, got {error:?}");
    };
    assert!(tail.ends_with("command not found"));
    assert!(tail.len() <= UNEXPECTED_OUTPUT_TAIL_BYTES);
}

// ---- read parser ----------------------------------------------------------------------------

fn read_report(status_lines: &str, body: &str) -> String {
    format!("{}{status_lines}{body}", frame("user root"))
}

#[test]
fn a_read_report_decodes_the_file_and_hashes_it() {
    let m = marker(NONCE);
    let output = read_report(
        &format!("{m} size 5\n{m} status ok\n"),
        &format!("aGVs\nbG8=\n\n{m} end\n"),
    );
    let ReadOutcome::Ok {
        bytes,
        sha256,
        user,
    } = parse_read(&output).unwrap()
    else {
        panic!("expected content");
    };
    assert_eq!(bytes, b"hello");
    assert_eq!(sha256, sha256_hex(b"hello"));
    assert_eq!(sha256.len(), 64);
    assert_eq!(user, "root");
}

#[test]
fn an_empty_file_reads_as_empty_content() {
    let m = marker(NONCE);
    let output = read_report(
        &format!("{m} size 0\n{m} status ok\n"),
        &format!("\n{m} end\n"),
    );
    let ReadOutcome::Ok { bytes, .. } = parse_read(&output).unwrap() else {
        panic!("expected content");
    };
    assert!(bytes.is_empty());
}

#[test]
fn a_missing_file_is_an_outcome_not_an_error() {
    assert_eq!(
        parse_read(&read_report(&frame("status not_found"), "")).unwrap(),
        ReadOutcome::NotFound
    );
}

#[test]
fn read_status_failures_become_errors() {
    for status in [
        "not_regular",
        "permission_denied",
        "missing_base64",
        "too_large",
    ] {
        let output = read_report(
            &format!(
                "{}{}",
                frame("size 999999"),
                frame(&format!("status {status}"))
            ),
            "",
        );
        assert!(
            matches!(parse_read(&output), Err(AgentBridgeError::RemoteFailed(_))),
            "{status}"
        );
    }
    let output = read_report(&frame("status surprise"), "");
    assert!(matches!(
        parse_read(&output),
        Err(AgentBridgeError::UnexpectedOutput(_))
    ));
}

#[test]
fn a_permission_error_names_the_user() {
    let output = read_report(&frame("status permission_denied"), "");
    let error = parse_read(&output).unwrap_err();
    assert!(error.to_string().contains("root"), "{error}");
}

#[test]
fn a_file_that_grew_past_the_limit_is_refused_even_if_the_size_said_otherwise() {
    let m = marker(NONCE);
    let big = BASE64.encode(vec![b'a'; 2000]);
    let output = read_report(
        &format!("{m} size 10\n{m} status ok\n"),
        &format!("{big}\n{m} end\n"),
    );
    assert!(matches!(
        parse_read(&output),
        Err(AgentBridgeError::RemoteFailed(_))
    ));
}

#[test]
fn a_file_that_changed_size_while_it_was_read_is_a_conflict() {
    let m = marker(NONCE);
    let output = read_report(
        &format!("{m} size 10\n{m} status ok\n"),
        &format!("aGk=\n{m} end\n"),
    );
    assert!(matches!(
        parse_read(&output),
        Err(AgentBridgeError::Conflict(_))
    ));
}

#[test]
fn content_that_is_cut_short_or_not_base64_is_an_error() {
    let m = marker(NONCE);
    let head = format!("{m} size 2\n{m} status ok\n");
    assert!(matches!(
        parse_read(&read_report(&head, "aGk=\n")),
        Err(AgentBridgeError::UnexpectedOutput(_))
    ));
    assert!(matches!(
        parse_read(&read_report(&head, &format!("!!!\n{m} end\n"))),
        Err(AgentBridgeError::UnexpectedOutput(_))
    ));
}

// ---- write parser ---------------------------------------------------------------------------

#[test]
fn a_new_file_report_says_it_was_created() {
    let output = format!(
        "{}{}{}",
        frame("created 1"),
        frame("sha256 abc"),
        frame("status ok")
    );
    assert_eq!(
        parse_write(&output).unwrap(),
        WriteOutcome {
            created: true,
            backup_path: None,
            sha256: "abc".to_owned()
        }
    );
}

#[test]
fn an_overwrite_report_carries_the_backup_path() {
    let output = format!(
        "{}{}{}",
        frame("backup /root/.warp-agent/backups/a.conf.1"),
        frame("sha256 def"),
        frame("status ok")
    );
    let outcome = parse_write(&output).unwrap();
    assert!(!outcome.created);
    assert_eq!(
        outcome.backup_path.as_deref(),
        Some("/root/.warp-agent/backups/a.conf.1")
    );
}

#[test]
fn write_errors_distinguish_conflicts_from_failures() {
    for code in ["already_exists", "changed_on_server"] {
        assert!(matches!(
            parse_write(&frame(&format!("error {code}"))),
            Err(AgentBridgeError::Conflict(_))
        ));
    }
    for code in [
        "not_found",
        "not_regular",
        "parent_missing",
        "missing_sha256",
        "backup_failed",
        "write_failed",
        "size_mismatch",
    ] {
        assert!(
            matches!(
                parse_write(&frame(&format!("error {code}"))),
                Err(AgentBridgeError::RemoteFailed(_))
            ),
            "{code}"
        );
    }
    assert!(matches!(
        parse_write(&frame("error other")),
        Err(AgentBridgeError::UnexpectedOutput(_))
    ));
}

#[test]
fn a_failed_write_after_the_backup_points_at_the_backup() {
    let output = format!(
        "{}{}",
        frame("backup /root/.warp-agent/backups/x"),
        frame("error write_failed")
    );
    let error = parse_write(&output).unwrap_err();
    assert!(
        error.to_string().contains("/root/.warp-agent/backups/x"),
        "{error}"
    );
}

#[test]
fn a_write_report_that_never_completes_is_an_error() {
    assert!(matches!(
        parse_write(&frame("sha256 abc")),
        Err(AgentBridgeError::UnexpectedOutput(_))
    ));
}

// ---- against a real shell -------------------------------------------------------------------

#[cfg(unix)]
mod with_sh {
    use std::fs;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::path::Path;
    use std::process::Output;

    use command::blocking::Command;

    use super::*;
    use crate::warp_sync::remote_script::upload_chunk_commands;

    fn run_sh(script: &str, home: Option<&Path>) -> Output {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        if let Some(home) = home {
            command.env("HOME", home);
        }
        command.output().expect("sh is available")
    }

    fn exec(
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u32,
    ) -> Result<ExecOutput, AgentBridgeError> {
        let nonce = new_nonce();
        let script = exec_script(&nonce, command, cwd, timeout_secs);
        let output = run_sh(&wrap_for_any_shell(&script), None);
        parse_exec_output(&nonce, &output.stdout, &output.stderr)
    }

    #[test]
    fn exec_captures_both_streams_and_the_exit_code() {
        let output = exec("echo out; echo err >&2; exit 3", None, 30).unwrap();
        assert_eq!(output.stdout.text, "out\n");
        assert_eq!(output.stderr.text, "err\n");
        assert_eq!(output.exit_code, 3);
        assert!(!output.timed_out);
    }

    #[test]
    fn exec_cuts_long_output_and_reports_its_size() {
        let output = exec("seq 1 200000", None, 60).unwrap();
        let expected_total: u64 = (1..=200_000u64)
            .map(|n| n.to_string().len() as u64 + 1)
            .sum();
        assert!(output.stdout.truncated);
        assert_eq!(output.stdout.total_bytes, expected_total);
        assert!(output.stdout.text.starts_with("1\n2\n"));
        assert!(output.stdout.text.ends_with("200000\n"));
        assert!(output.stdout.text.len() < EXEC_STDOUT_MAX_BYTES + 100);
        assert!(output.stdout.text.contains("bytes omitted"));
    }

    #[test]
    fn a_command_cannot_read_the_script_from_stdin() {
        let output = exec("read x; echo got=$x", None, 30).unwrap();
        assert_eq!(output.stdout.text, "got=\n");
        assert_eq!(output.exit_code, 0);
    }

    #[test]
    fn exec_stops_a_command_that_outlives_its_timeout() {
        if Command::new("timeout").arg("--version").output().is_err() {
            return;
        }
        let output = exec("sleep 5", None, 1).unwrap();
        assert!(output.timed_out);
        assert_eq!(output.exit_code, TIMEOUT_EXIT_CODE);
    }

    #[test]
    fn exec_runs_in_the_requested_directory_without_moving_the_caller() {
        let before = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        let output = exec("pwd; cd /", Some(path.to_str().unwrap()), 30).unwrap();
        assert_eq!(output.stdout.text.trim(), path.to_str().unwrap());
        assert_eq!(std::env::current_dir().unwrap(), before);
    }

    #[test]
    fn a_hostile_cwd_neither_runs_nor_is_accepted() {
        let scratch = tempfile::tempdir().unwrap();
        let proof = scratch.path().join("proof");
        let cwd = format!("/nonexistent'; touch {}; '", proof.display());
        let error = exec("true", Some(&cwd), 30).unwrap_err();
        assert!(matches!(error, AgentBridgeError::InvalidParams(_)));
        assert!(!proof.exists());
    }

    #[test]
    fn exec_leaves_no_scratch_directory_behind() {
        let scratch = tempfile::tempdir().unwrap();
        let nonce = new_nonce();
        let script = exec_script(&nonce, "echo hi", None, 30);
        let mut command = Command::new("sh");
        command.args(["-c", &wrap_for_any_shell(&script)]);
        command.env("TMPDIR", scratch.path());
        let output = command.output().unwrap();
        assert!(parse_exec_output(&nonce, &output.stdout, &output.stderr).is_ok());
        assert!(fs::read_dir(scratch.path()).unwrap().next().is_none());
    }

    fn read(path: &Path, max_bytes: usize) -> Result<ReadOutcome, AgentBridgeError> {
        let nonce = new_nonce();
        let script = read_script(&nonce, path.to_str().unwrap(), max_bytes);
        let output = run_sh(&script, None);
        parse_read_output(&nonce, &output.stdout, &output.stderr, max_bytes)
    }

    #[test]
    fn read_returns_the_content_of_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, b"line 1\n\xff\x00binary").unwrap();
        let ReadOutcome::Ok { bytes, sha256, .. } = read(&path, 1024).unwrap() else {
            panic!("expected content");
        };
        assert_eq!(bytes, b"line 1\n\xff\x00binary");
        assert_eq!(sha256, sha256_hex(&bytes));
    }

    #[test]
    fn read_reports_a_missing_file_a_directory_and_an_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read(&dir.path().join("nope"), 1024).unwrap(),
            ReadOutcome::NotFound
        );
        assert!(matches!(
            read(dir.path(), 1024),
            Err(AgentBridgeError::RemoteFailed(message)) if message.contains("regular file")
        ));
        let big = dir.path().join("big");
        fs::write(&big, vec![b'x'; 100]).unwrap();
        assert!(matches!(
            read(&big, 10),
            Err(AgentBridgeError::RemoteFailed(message)) if message.contains("limit")
        ));
        assert!(matches!(read(&big, 100), Ok(ReadOutcome::Ok { .. })));
    }

    #[test]
    fn read_reports_an_unreadable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        fs::write(&path, "x").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(&path).is_ok() {
            return; // running as root: mode bits do not apply
        }
        assert!(matches!(
            read(&path, 1024),
            Err(AgentBridgeError::RemoteFailed(message)) if message.contains("Permission denied")
        ));
    }

    /// Stages `content` in a scratch directory the way `ops` does and returns it.
    fn stage(content: &[u8]) -> (tempfile::TempDir, RemoteTmpDir) {
        let scratch = tempfile::Builder::new()
            .prefix("warp-sync.")
            .tempdir()
            .unwrap();
        let dir = validate_tmp_dir(scratch.path().to_str().unwrap()).unwrap();
        for command in upload_chunk_commands(&dir, content) {
            assert!(run_sh(&command, None).status.success());
        }
        (scratch, dir)
    }

    fn write(
        path: &Path,
        content: &[u8],
        expectation: &WriteExpectation,
        home: &Path,
    ) -> Result<WriteOutcome, AgentBridgeError> {
        let (_scratch, dir) = stage(content);
        let nonce = new_nonce();
        let script = write_commit_script(
            &nonce,
            &dir,
            path.to_str().unwrap(),
            expectation,
            content.len(),
            "backup-1",
        );
        let output = run_sh(&script, Some(home));
        parse_write_output(&nonce, &output.stdout, &output.stderr)
    }

    fn must_match(content: &[u8]) -> WriteExpectation {
        WriteExpectation::MustMatch {
            sha256: sha256_hex(content),
        }
    }

    #[test]
    fn must_not_exist_creates_a_new_file() {
        let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let path = dir.path().join("new.conf");
        let outcome = write(
            &path,
            b"fresh\n",
            &WriteExpectation::MustNotExist,
            home.path(),
        )
        .unwrap();
        assert!(outcome.created);
        assert_eq!(outcome.backup_path, None);
        assert_eq!(outcome.sha256, sha256_hex(b"fresh\n"));
        assert_eq!(fs::read(&path).unwrap(), b"fresh\n");
    }

    #[test]
    fn must_not_exist_refuses_an_existing_file_and_a_dangling_symlink() {
        let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let existing = dir.path().join("there");
        fs::write(&existing, "keep").unwrap();
        assert!(matches!(
            write(
                &existing,
                b"x",
                &WriteExpectation::MustNotExist,
                home.path()
            ),
            Err(AgentBridgeError::Conflict(_))
        ));
        assert_eq!(fs::read_to_string(&existing).unwrap(), "keep");

        let elsewhere = dir.path().join("elsewhere");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        assert!(matches!(
            write(&link, b"x", &WriteExpectation::MustNotExist, home.path()),
            Err(AgentBridgeError::Conflict(_))
        ));
        assert!(!elsewhere.exists());
    }

    #[test]
    fn must_not_exist_needs_the_parent_directory() {
        let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let path = dir.path().join("missing").join("f");
        assert!(matches!(
            write(&path, b"x", &WriteExpectation::MustNotExist, home.path()),
            Err(AgentBridgeError::RemoteFailed(message)) if message.contains("parent")
        ));
    }

    #[test]
    fn must_match_overwrites_in_place_keeping_the_mode_and_saves_a_backup() {
        let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let path = dir.path().join("app.conf");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let inode = fs::metadata(&path).unwrap().ino();

        let outcome = write(&path, b"new\n", &must_match(b"old\n"), home.path()).unwrap();

        assert!(!outcome.created);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(outcome.sha256, sha256_hex(b"new\n"));
        let backup = outcome.backup_path.expect("an overwrite is backed up");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "old\n");
        assert!(backup.starts_with(home.path().join(".warp-agent/backups").to_str().unwrap()));
        assert_eq!(
            fs::metadata(home.path().join(".warp-agent/backups"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn must_match_with_a_stale_checksum_changes_nothing() {
        let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let path = dir.path().join("app.conf");
        fs::write(&path, "changed by someone else\n").unwrap();
        let result = write(&path, b"new\n", &must_match(b"what I read\n"), home.path());
        assert!(matches!(result, Err(AgentBridgeError::Conflict(_))));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "changed by someone else\n"
        );
        assert!(!home.path().join(".warp-agent").exists());
    }

    #[test]
    fn must_match_needs_an_existing_regular_file() {
        let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert!(matches!(
            write(
                &dir.path().join("gone"),
                b"x",
                &must_match(b"y"),
                home.path()
            ),
            Err(AgentBridgeError::RemoteFailed(_))
        ));
        assert!(matches!(
            write(dir.path(), b"x", &must_match(b"y"), home.path()),
            Err(AgentBridgeError::RemoteFailed(_))
        ));
    }

    #[test]
    fn must_match_refuses_a_file_in_a_directory_everyone_can_write_to() {
        let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let path = dir.path().join("app.conf");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).unwrap();

        let result = write(&path, b"new\n", &must_match(b"old\n"), home.path());

        assert!(
            matches!(&result, Err(AgentBridgeError::RemoteFailed(message)) if message.contains("writable by everyone")),
            "{result:?}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
        assert!(!home.path().join(".warp-agent").exists());
    }

    #[test]
    fn must_match_refuses_to_back_up_through_a_symlinked_backup_directory() {
        let (dir, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), home.path().join(".warp-agent")).unwrap();
        let path = dir.path().join("app.conf");
        fs::write(&path, "old\n").unwrap();

        let result = write(&path, b"new\n", &must_match(b"old\n"), home.path());

        assert!(matches!(result, Err(AgentBridgeError::RemoteFailed(_))));
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
        assert!(fs::read_dir(elsewhere.path()).unwrap().next().is_none());
    }

    #[test]
    fn a_payload_of_the_wrong_size_is_refused_and_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let (scratch, staged) = stage(b"abc");
        let nonce = new_nonce();
        let script = write_commit_script(
            &nonce,
            &staged,
            dir.path().join("f").to_str().unwrap(),
            &WriteExpectation::MustNotExist,
            99,
            "b",
        );
        let output = run_sh(&script, Some(home.path()));
        let result = parse_write_output(&nonce, &output.stdout, &output.stderr);
        assert!(matches!(result, Err(AgentBridgeError::RemoteFailed(_))));
        assert!(!dir.path().join("f").exists());
        assert!(!scratch.path().exists());
    }
}
