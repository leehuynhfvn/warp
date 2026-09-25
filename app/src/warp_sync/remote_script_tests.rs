use super::*;

const NASTY_PATHS: [&str; 5] = [
    "/tmp/$(rm -rf /)",
    "/tmp/`id`",
    "/tmp/it's",
    "/tmp/a b;c|d&e",
    "/tmp/-rf",
];

fn probe_output(lines: &[&str]) -> String {
    lines.join("\n")
}

fn full_probe(extra: &[&str]) -> String {
    let mut lines = vec![
        "status=ok",
        "user=root",
        "uid=0",
        "kind=dir",
        "size_kib=12",
        "tar=gnu",
        "base64=yes",
        "machine_id=0123456789abcdef0123456789abcdef",
    ];
    lines.extend_from_slice(extra);
    probe_output(&lines)
}

fn tmp_dir(path: &str) -> RemoteTmpDir {
    validate_tmp_dir(path).unwrap()
}

#[test]
fn posix_quote_wraps_plain_text() {
    assert_eq!(posix_quote("/etc/hosts"), "'/etc/hosts'");
}

#[test]
fn posix_quote_escapes_single_quotes() {
    assert_eq!(posix_quote("it's"), r"'it'\''s'");
}

#[test]
fn wrap_for_any_shell_has_the_expected_shape_and_round_trips() {
    let script = "echo 'hi'\necho \"$HOME\"\n";
    let wrapped = wrap_for_any_shell(script);

    let regex = Regex::new(r"^printf %s ([A-Za-z0-9+/=]+) \| base64 -d \| sh$").unwrap();
    let captures = regex.captures(&wrapped).expect("wrapped command shape");
    let decoded = BASE64.decode(&captures[1]).unwrap();
    assert_eq!(decoded, script.as_bytes());
}

#[test]
fn probe_script_only_uses_the_path_quoted() {
    for path in NASTY_PATHS {
        let script = probe_script(path);
        let quoted = posix_quote(path);
        assert!(script.starts_with(&format!("P={quoted}\n")), "{path}");
        assert!(!script.replace(&quoted, "").contains(path), "{path}");
    }
}

#[test]
fn download_script_only_uses_the_path_quoted() {
    for name in NASTY_PATHS {
        let script = download_script("/srv/data", name);
        let quoted = posix_quote(&format!("./{name}"));
        assert!(
            script.contains(&format!("-C '/srv/data' {quoted} ")),
            "{name}"
        );
        assert!(!script.replace(&quoted, "").contains(name), "{name}");
    }
}

#[test]
fn download_script_protects_leading_dash_names() {
    assert!(download_script("/tmp", "-rf").contains("'./-rf'"));
}

#[test]
fn upload_begin_command_is_wrapped() {
    let command = upload_begin_command();
    assert!(command.starts_with("printf %s "));
    assert!(command.ends_with(" | base64 -d | sh"));
}

#[test]
fn validate_tmp_dir_accepts_mktemp_output() {
    assert_eq!(
        validate_tmp_dir("/tmp/warp-sync.AbC123\r\n")
            .unwrap()
            .as_str(),
        "/tmp/warp-sync.AbC123"
    );
    assert!(validate_tmp_dir("/var/tmp/my-dir/warp-sync.Xy9Z").is_ok());
}

#[test]
fn validate_tmp_dir_rejects_suspicious_paths() {
    for path in [
        "/tmp/x; rm -rf /",
        "/tmp/warp-sync.",
        "tmp/warp-sync.AbC123",
        "warp-sync.AbC123",
        "/tmp/../warp-sync.AbC123",
        "/tmp/warp-sync.AbC123 /etc",
        "/tmp/warp-sync.$(id)",
        "",
    ] {
        assert!(
            validate_tmp_dir(path).is_err(),
            "{path:?} should be rejected"
        );
    }
}

#[test]
fn upload_chunks_reassemble_and_are_aligned() {
    let tgz: Vec<u8> = (0..60_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let dir = tmp_dir("/tmp/warp-sync.AbC123");

    let commands = upload_chunk_commands(&dir, &tgz);

    assert!(commands.len() > 1);
    let mut reassembled = Vec::new();
    for command in &commands {
        let words: Vec<&str> = command.split(' ').collect();
        assert_eq!(&words[..2], ["printf", "%s"]);
        let chunk = words[2];
        assert!(chunk.len() <= UPLOAD_CHUNK_B64_LEN);
        assert_eq!(chunk.len() % 4, 0);
        assert_eq!(
            &words[3..],
            [
                "|",
                "base64",
                "-d",
                ">>",
                "'/tmp/warp-sync.AbC123/payload.tgz'"
            ]
        );
        reassembled.extend(BASE64.decode(chunk).unwrap());
    }
    assert_eq!(reassembled, tgz);
}

#[test]
fn cleanup_command_removes_only_the_quoted_dir() {
    let dir = tmp_dir("/tmp/warp-sync.AbC123");
    assert_eq!(cleanup_command(&dir), "rm -rf '/tmp/warp-sync.AbC123'");
}

#[test]
fn extract_mode_depends_on_tar_flavor_and_uid() {
    let mut probe = parse_probe_output(&full_probe(&[])).unwrap();
    assert_eq!(
        ExtractMode::for_probe(&probe),
        ExtractMode::GnuPreserveOwner
    );

    probe.uid = 1000;
    assert_eq!(ExtractMode::for_probe(&probe), ExtractMode::GnuNoOwner);

    probe.tar = TarFlavor::Other;
    assert_eq!(ExtractMode::for_probe(&probe), ExtractMode::Generic);
}

#[test]
fn commit_script_embeds_paths_quoted_and_flags() {
    let dir = tmp_dir("/tmp/warp-sync.AbC123");
    let script = upload_commit_script(&UploadCommit {
        tmp_dir: &dir,
        parent: "/etc/it's",
        name: "$(id)",
        expected_len: 4242,
        backup_name: "prod-1_etc-1727000000",
        extract_mode: ExtractMode::GnuPreserveOwner,
    });

    assert!(script.contains(r"P='/etc/it'\''s'"));
    assert!(script.contains("N='./$(id)'"));
    assert!(script.contains(r#"= "4242" ]"#));
    assert!(script.contains("-p --same-owner --numeric-owner"));
    assert!(script.contains("'prod-1_etc-1727000000.tgz'"));
    assert_eq!(script.matches("$(id)").count(), 1);
}

#[test]
fn probe_parses_a_complete_report() {
    let probe = parse_probe_output(&full_probe(&[])).unwrap();
    assert_eq!(
        probe,
        ProbeResult {
            status: ProbeStatus::Ok,
            user: "root".to_owned(),
            uid: 0,
            kind: RemoteKind::Dir,
            size_kib: Some(12),
            tar: TarFlavor::Gnu,
            has_base64: true,
            machine_id: Some("0123456789abcdef0123456789abcdef".to_owned()),
        }
    );
}

#[test]
fn probe_without_a_valid_machine_id_reports_none() {
    for value in ["", "short", "has space in it 12345", "$(id)$(id)$(id)"] {
        let output = full_probe(&[&format!("machine_id={value}")]);
        assert_eq!(
            parse_probe_output(&output).unwrap().machine_id,
            None,
            "{value:?}"
        );
    }
}

#[test]
fn probe_ignores_noise_lines() {
    let output = full_probe(&["Last login: Tue", "motd without equals", "=novalue"]);
    assert!(parse_probe_output(&output).is_ok());
}

#[test]
fn probe_reports_not_found_without_other_fields() {
    let probe = parse_probe_output("status=not_found\n").unwrap();
    assert_eq!(probe.status, ProbeStatus::NotFound);
}

#[test]
fn probe_reports_permission_denied_with_the_user() {
    let output = probe_output(&[
        "status=permission_denied",
        "user=alice",
        "uid=1000",
        "kind=file",
        "size_kib=",
        "tar=other",
        "base64=no",
    ]);
    let probe = parse_probe_output(&output).unwrap();
    assert_eq!(probe.status, ProbeStatus::PermissionDenied);
    assert_eq!(probe.user, "alice");
    assert_eq!(probe.tar, TarFlavor::Other);
    assert!(!probe.has_base64);
}

#[test]
fn probe_with_empty_size_has_no_size() {
    let output = full_probe(&[]).replace("size_kib=12", "size_kib=");
    assert_eq!(parse_probe_output(&output).unwrap().size_kib, None);
}

#[test]
fn probe_missing_a_field_is_an_error() {
    for field in ["status", "user", "uid", "kind", "tar", "base64"] {
        let output: String = full_probe(&[])
            .lines()
            .filter(|line| !line.starts_with(&format!("{field}=")))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            parse_probe_output(&output).is_err(),
            "missing {field} should fail"
        );
    }
}

#[test]
fn probe_with_unknown_status_is_an_error() {
    assert!(parse_probe_output("status=weird\n").is_err());
}

#[test]
fn failure_message_is_the_last_non_empty_line() {
    let output = b"tar: a: Cannot open: Permission denied\ntar: Exiting with failure status\n\r\n";
    assert_eq!(
        remote_failure_message(output),
        "tar: Exiting with failure status"
    );
}

#[test]
fn failure_message_only_considers_the_tail() {
    let mut output = b"first line\n".to_vec();
    output.extend(std::iter::repeat_n(b'x', 5000));
    let message = remote_failure_message(&output);
    assert_eq!(message.len(), FAILURE_MESSAGE_TAIL_BYTES);
    assert!(!message.contains("first"));
}

#[test]
fn failure_message_of_empty_output_is_empty() {
    assert_eq!(remote_failure_message(b""), "");
}

#[cfg(unix)]
mod with_sh {
    use std::fs;
    use std::io::Read as _;
    use std::path::Path;
    use std::process::Output;

    use command::blocking::Command;

    use super::*;

    fn run_sh(script: &str, home: Option<&Path>) -> Output {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        if let Some(home) = home {
            command.env("HOME", home);
        }
        command.output().expect("sh is available")
    }

    fn stdout(output: &Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn make_tgz(name: &str, contents: &str) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o600);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, format!("./{name}"), contents.as_bytes())
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn wrapped_script_runs_identically() {
        let script = "printf '%s' \"it's $((1 + 2))\"\n";
        let output = run_sh(&wrap_for_any_shell(script), None);
        assert_eq!(stdout(&output), "it's 3");
    }

    #[test]
    fn probe_script_describes_a_directory_with_a_nasty_name() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("we ird'$(touch pwned)`x`");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("f"), "data").unwrap();

        let output = run_sh(&probe_script(dir.to_str().unwrap()), None);
        let probe = parse_probe_output(&stdout(&output)).unwrap();

        assert_eq!(probe.status, ProbeStatus::Ok);
        assert_eq!(probe.kind, RemoteKind::Dir);
        assert!(probe.has_base64);
        assert!(!dir.join("pwned").exists());
    }

    #[test]
    fn probe_script_reports_a_missing_path() {
        let output = run_sh(&probe_script("/definitely/not/here"), None);
        let probe = parse_probe_output(&stdout(&output)).unwrap();
        assert_eq!(probe.status, ProbeStatus::NotFound);
    }

    #[test]
    fn download_script_emits_a_readable_tarball() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("-conf")).unwrap();
        fs::write(temp.path().join("-conf/a b.txt"), "hello").unwrap();

        let script = download_script(temp.path().to_str().unwrap(), "-conf");
        let output = run_sh(&script, None);
        assert!(output.status.success());

        let decoder = flate2::read::GzDecoder::new(output.stdout.as_slice());
        let mut archive = tar::Archive::new(decoder);
        let mut found = false;
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().ends_with("a b.txt") {
                let mut contents = String::new();
                entry.read_to_string(&mut contents).unwrap();
                assert_eq!(contents, "hello");
                found = true;
            }
        }
        assert!(found);
    }

    #[test]
    fn download_script_failure_prints_the_reason_and_keeps_the_exit_code() {
        let temp = tempfile::tempdir().unwrap();
        let script = download_script(temp.path().to_str().unwrap(), "missing");
        let output = run_sh(&script, None);

        assert!(!output.status.success());
        assert!(!remote_failure_message(&output.stdout).is_empty());
    }

    #[test]
    fn chunks_and_commit_replace_a_file_and_back_it_up() {
        let scratch = tempfile::Builder::new()
            .prefix("warp-sync.")
            .tempdir()
            .unwrap();
        let dir = validate_tmp_dir(scratch.path().to_str().unwrap()).unwrap();
        let target_parent = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        fs::write(target_parent.path().join("conf"), "old").unwrap();

        let tgz = make_tgz("conf", "new contents");
        for command in upload_chunk_commands(&dir, &tgz) {
            assert!(run_sh(&command, None).status.success());
        }
        let commit = upload_commit_script(&UploadCommit {
            tmp_dir: &dir,
            parent: target_parent.path().to_str().unwrap(),
            name: "conf",
            expected_len: tgz.len(),
            backup_name: "host_conf-1",
            extract_mode: ExtractMode::Generic,
        });
        let output = run_sh(&commit, Some(home.path()));

        assert!(output.status.success(), "{}", stdout(&output));
        assert_eq!(
            fs::read_to_string(target_parent.path().join("conf")).unwrap(),
            "new contents"
        );
        let backup = home.path().join(".warp-sync/backups/host_conf-1.tgz");
        assert!(backup.exists());
        assert!(stdout(&output).contains(&format!("backup={}", backup.display())));
        assert!(!scratch.path().exists());
    }

    #[test]
    fn commit_refuses_to_back_up_through_a_symlinked_backup_directory() {
        let scratch = tempfile::Builder::new()
            .prefix("warp-sync.")
            .tempdir()
            .unwrap();
        let dir = validate_tmp_dir(scratch.path().to_str().unwrap()).unwrap();
        let target_parent = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        fs::write(target_parent.path().join("conf"), "old").unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), home.path().join(".warp-sync")).unwrap();
        let tgz = make_tgz("conf", "new");
        for command in upload_chunk_commands(&dir, &tgz) {
            assert!(run_sh(&command, None).status.success());
        }

        let commit = upload_commit_script(&UploadCommit {
            tmp_dir: &dir,
            parent: target_parent.path().to_str().unwrap(),
            name: "conf",
            expected_len: tgz.len(),
            backup_name: "b",
            extract_mode: ExtractMode::Generic,
        });
        let output = run_sh(&commit, Some(home.path()));

        assert_eq!(output.status.code(), Some(EXIT_BACKUP_DIR));
        assert_eq!(
            fs::read_to_string(target_parent.path().join("conf")).unwrap(),
            "old"
        );
        assert!(fs::read_dir(elsewhere.path()).unwrap().next().is_none());
    }

    #[test]
    fn commit_rejects_a_payload_of_the_wrong_size() {
        let scratch = tempfile::Builder::new()
            .prefix("warp-sync.")
            .tempdir()
            .unwrap();
        let dir = validate_tmp_dir(scratch.path().to_str().unwrap()).unwrap();
        let target_parent = tempfile::tempdir().unwrap();
        let tgz = make_tgz("conf", "x");
        for command in upload_chunk_commands(&dir, &tgz) {
            assert!(run_sh(&command, None).status.success());
        }

        let commit = upload_commit_script(&UploadCommit {
            tmp_dir: &dir,
            parent: target_parent.path().to_str().unwrap(),
            name: "conf",
            expected_len: tgz.len() + 1,
            backup_name: "b",
            extract_mode: ExtractMode::Generic,
        });
        let output = run_sh(&commit, None);

        assert_eq!(output.status.code(), Some(EXIT_SIZE_MISMATCH));
        assert!(!target_parent.path().join("conf").exists());
    }

    #[test]
    fn checksum_script_hashes_every_regular_file_on_a_real_shell() {
        use sha2::{Digest, Sha256};

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("a b.conf"), "one").unwrap();
        fs::write(dir.path().join("sub/c.conf"), "two").unwrap();
        let root = dir.path().to_str().unwrap();

        let output = run_sh(&checksum_script(root), None);
        let parsed = parse_checksum_output(&String::from_utf8_lossy(&output.stdout));

        let Some(parsed) = parsed else {
            return; // The machine running the tests has neither sha256sum nor shasum.
        };
        assert!(output.status.success());
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[&format!("{root}/a b.conf")],
            hex::encode(Sha256::digest(b"one"))
        );
        assert_eq!(
            parsed[&format!("{root}/sub/c.conf")],
            hex::encode(Sha256::digest(b"two"))
        );
    }
}

#[test]
fn checksum_script_only_uses_the_path_quoted() {
    for path in NASTY_PATHS {
        let script = checksum_script(path);
        let quoted = posix_quote(path);
        assert!(script.starts_with(&format!("P={quoted}\n")), "{path}");
        assert!(!script.replace(&quoted, "").contains(path), "{path}");
    }
}

#[test]
fn parse_checksum_output_reads_hash_and_path() {
    let output = format!(
        "{a}  /etc/nginx/nginx.conf\n{b} */etc/with space.conf\n",
        a = "A".repeat(64),
        b = "b".repeat(64)
    );

    let parsed = parse_checksum_output(&output).unwrap();

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed["/etc/nginx/nginx.conf"], "a".repeat(64));
    assert_eq!(parsed["/etc/with space.conf"], "b".repeat(64));
}

#[test]
fn parse_checksum_output_reports_a_missing_hash_tool() {
    assert_eq!(parse_checksum_output("no_hash_tool\n"), None);
}

#[test]
fn parse_checksum_output_skips_lines_that_are_not_hashes() {
    let output = format!(
        "sha256sum: /root/x: Permission denied\n\\{h}  /odd\\nname\n{h}  \n{short}  /a\n",
        h = "c".repeat(64),
        short = "d".repeat(63)
    );

    assert_eq!(parse_checksum_output(&output), Some(BTreeMap::new()));
}
