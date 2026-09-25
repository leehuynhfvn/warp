use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use command::blocking::Command;
use futures::executor::block_on;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::super::remote_check::RemoteConflicts;
use super::super::remote_script::remote_failure_message;
use super::*;

const HOST_KEY: &str = "h";

/// A "remote host" that is just this machine: commands run in a local `sh`, with `HOME` and
/// `TMPDIR` redirected into a sandbox.
struct LocalSh {
    home: PathBuf,
    tmpdir: PathBuf,
    calls: AtomicUsize,
    /// 1-based number of the call that fails without running; 0 for none.
    fail_call: AtomicUsize,
}

impl LocalSh {
    fn fail_on_call_from_now(&self, call: usize) {
        self.calls.store(0, Ordering::SeqCst);
        self.fail_call.store(call, Ordering::SeqCst);
    }
}

#[async_trait]
impl RemoteShell for LocalSh {
    async fn run(&self, command: &str) -> Result<Vec<u8>, WarpSyncError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.fail_call.load(Ordering::SeqCst) {
            return Err(WarpSyncError::Executor("injected failure".to_owned()));
        }
        let output = Command::new("sh")
            .args(["-c", command])
            .env("HOME", &self.home)
            .env("TMPDIR", &self.tmpdir)
            .output()
            .map_err(|err| WarpSyncError::Executor(err.to_string()))?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let source = if output.stderr.is_empty() {
            &output.stdout
        } else {
            &output.stderr
        };
        Err(WarpSyncError::RemoteCommandFailed {
            exit_code: output.status.code(),
            message: remote_failure_message(source),
        })
    }
}

/// The same "remote host" as [`LocalSh`], but reporting a chosen machine id.
struct AsMachine<'a> {
    shell: &'a LocalSh,
    machine_id: &'a str,
}

#[async_trait]
impl RemoteShell for AsMachine<'_> {
    async fn run(&self, command: &str) -> Result<Vec<u8>, WarpSyncError> {
        let output = self.shell.run(command).await?;
        let text = String::from_utf8_lossy(&output);
        if !text.contains("status=") {
            return Ok(output);
        }
        let mut lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.starts_with("machine_id="))
            .collect();
        let machine_line = format!("machine_id={}", self.machine_id);
        lines.push(&machine_line);
        Ok(lines.join("\n").into_bytes())
    }
}

/// The same "remote host" as [`LocalSh`], but one that has no tool to hash files.
struct WithoutHashTool<'a>(&'a LocalSh);

#[async_trait]
impl RemoteShell for WithoutHashTool<'_> {
    async fn run(&self, command: &str) -> Result<Vec<u8>, WarpSyncError> {
        use base64::Engine as _;
        let script = command
            .split_whitespace()
            .nth(2)
            .and_then(|encoded| base64::engine::general_purpose::STANDARD.decode(encoded).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        if script.contains("-exec $H") {
            return Ok(b"no_hash_tool\n".to_vec());
        }
        self.0.run(command).await
    }
}

struct Env {
    dir: TempDir,
    shell: LocalSh,
    remote_path: String,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("remote/conf");
        fs::create_dir_all(remote.join("sub")).unwrap();
        fs::write(remote.join("a.conf"), "remote a").unwrap();
        fs::write(remote.join("b.conf"), "remote b").unwrap();
        fs::write(remote.join("sub/c.conf"), "remote c").unwrap();
        fs::create_dir_all(dir.path().join("home")).unwrap();
        fs::create_dir_all(dir.path().join("tmp")).unwrap();
        let shell = LocalSh {
            home: dir.path().join("home"),
            tmpdir: dir.path().join("tmp"),
            calls: AtomicUsize::new(0),
            fail_call: AtomicUsize::new(0),
        };
        let remote_path = remote.to_str().unwrap().to_owned();
        Self {
            dir,
            shell,
            remote_path,
        }
    }

    fn remote(&self, relative: &str) -> PathBuf {
        Path::new(&self.remote_path).join(relative)
    }

    fn mirror_root(&self) -> PathBuf {
        self.dir.path().join("mirror")
    }

    fn local(&self, relative: &str) -> PathBuf {
        local_path_for(&self.mirror_root(), HOST_KEY, &self.remote_path).join(relative)
    }

    fn download_request(&self, allow_overwrite_local_changes: bool) -> DownloadRequest {
        DownloadRequest {
            remote_path: self.remote_path.clone(),
            host_key: HOST_KEY.to_owned(),
            mirror_root: self.mirror_root(),
            limits: SyncLimits::default(),
            allow_overwrite_local_changes,
        }
    }

    fn upload_request(&self) -> UploadRequest {
        UploadRequest {
            remote_path: self.remote_path.clone(),
            host_key: HOST_KEY.to_owned(),
            mirror_root: self.mirror_root(),
            limits: SyncLimits::default(),
        }
    }

    fn download(
        &self,
        allow_overwrite_local_changes: bool,
    ) -> Result<DownloadResult, WarpSyncError> {
        block_on(download(
            &self.shell,
            &self.download_request(allow_overwrite_local_changes),
        ))
    }

    fn download_done(&self) -> DownloadOutcome {
        match self.download(false).unwrap() {
            DownloadResult::Done(outcome) => outcome,
            DownloadResult::NeedsConfirmation { modified_files } => {
                panic!("unexpected confirmation: {modified_files:?}")
            }
        }
    }

    fn upload(&self) -> Result<UploadOutcome, WarpSyncError> {
        let prepared = block_on(prepare_upload(&self.shell, &self.upload_request()))?;
        block_on(execute_upload(&self.shell, &prepared))
    }

    fn prepare(&self) -> PreparedUpload {
        block_on(prepare_upload(&self.shell, &self.upload_request())).unwrap()
    }

    fn manifest(&self) -> Manifest {
        load_manifest(&self.mirror_root(), HOST_KEY).unwrap()
    }

    fn leftover_scratch_dirs(&self) -> Vec<String> {
        fs::read_dir(self.dir.path().join("tmp"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }
}

/// Whether the machine running the tests can hash files the way a remote host would.
fn has_hash_tool() -> bool {
    ["sha256sum", "shasum"].iter().any(|tool| {
        Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

fn conflicts_of(prepared: &PreparedUpload) -> RemoteConflicts {
    match &prepared.remote_check {
        RemoteCheck::Checked(conflicts) => conflicts.clone(),
        RemoteCheck::Unavailable => panic!("the remote check was unavailable"),
    }
}

fn sha256_hex(data: &str) -> String {
    hex::encode(Sha256::digest(data.as_bytes()))
}

fn running_as_root() -> bool {
    let probe = tempfile::tempdir().unwrap();
    let file = probe.path().join("f");
    fs::write(&file, "x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    }
    fs::read(&file).is_ok()
}

#[test]
fn download_mirrors_a_directory_and_records_the_manifest() {
    let env = Env::new();

    let outcome = env.download_done();

    assert_eq!((outcome.files, outcome.dirs), (3, 2));
    assert!(!outcome.remote_user.is_empty());
    assert_eq!(outcome.local_path, env.local(""));
    assert_eq!(fs::read_to_string(env.local("a.conf")).unwrap(), "remote a");
    assert_eq!(
        fs::read_to_string(env.local("sub/c.conf")).unwrap(),
        "remote c"
    );

    let manifest = env.manifest();
    let a = manifest
        .entry(&format!("{}/a.conf", env.remote_path))
        .unwrap();
    assert_eq!(a.sha256.as_deref(), Some(sha256_hex("remote a").as_str()));
    assert!(manifest.entry(&env.remote_path).is_some());
    assert!(manifest.last_sync(&env.remote_path).is_some());
}

#[test]
fn download_of_a_single_file() {
    let env = Env::new();
    let request = DownloadRequest {
        remote_path: format!("{}/a.conf", env.remote_path),
        ..env.download_request(false)
    };

    let result = block_on(download(&env.shell, &request)).unwrap();

    assert!(matches!(result, DownloadResult::Done(ref outcome) if outcome.files == 1));
    assert_eq!(fs::read_to_string(env.local("a.conf")).unwrap(), "remote a");
}

#[test]
fn download_of_a_missing_path_is_not_found() {
    let env = Env::new();
    let request = DownloadRequest {
        remote_path: format!("{}/nope", env.remote_path),
        ..env.download_request(false)
    };

    let result = block_on(download(&env.shell, &request));

    assert!(matches!(result, Err(WarpSyncError::NotFound(_))));
    assert!(!env.mirror_root().join(HOST_KEY).exists());
}

#[cfg(unix)]
#[test]
fn download_of_an_unreadable_file_is_permission_denied_and_creates_nothing() {
    use std::os::unix::fs::PermissionsExt;

    if running_as_root() {
        return;
    }
    let env = Env::new();
    fs::set_permissions(env.remote("a.conf"), fs::Permissions::from_mode(0o000)).unwrap();
    let request = DownloadRequest {
        remote_path: format!("{}/a.conf", env.remote_path),
        ..env.download_request(false)
    };

    let result = block_on(download(&env.shell, &request));

    assert!(matches!(
        result,
        Err(WarpSyncError::PermissionDenied { .. })
    ));
    assert!(!env.local("a.conf").exists());
}

#[test]
fn download_asks_before_discarding_local_edits() {
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "my edit").unwrap();
    fs::write(env.local("mine.conf"), "untracked").unwrap();

    let result = env.download(false).unwrap();

    let DownloadResult::NeedsConfirmation { modified_files } = result else {
        panic!("expected a confirmation request");
    };
    assert_eq!(
        modified_files,
        [
            format!("{}/a.conf", env.remote_path),
            format!("{}/mine.conf", env.remote_path)
        ]
    );
    assert_eq!(fs::read_to_string(env.local("a.conf")).unwrap(), "my edit");

    let result = env.download(true).unwrap();

    assert!(matches!(result, DownloadResult::Done(_)));
    assert_eq!(fs::read_to_string(env.local("a.conf")).unwrap(), "remote a");
    assert!(!env.local("mine.conf").exists());
}

#[test]
fn redownload_drops_files_deleted_on_the_remote_host() {
    let env = Env::new();
    env.download_done();
    fs::remove_file(env.remote("b.conf")).unwrap();
    fs::write(env.remote("a.conf"), "remote a v2").unwrap();

    env.download_done();

    assert!(!env.local("b.conf").exists());
    assert_eq!(
        fs::read_to_string(env.local("a.conf")).unwrap(),
        "remote a v2"
    );
    let manifest = env.manifest();
    assert!(
        manifest
            .entry(&format!("{}/b.conf", env.remote_path))
            .is_none()
    );
}

#[test]
fn download_leaves_no_staging_directory_behind() {
    let env = Env::new();
    env.download_done();

    let staging = env.mirror_root().join(".warp-sync/staging");
    let leftovers = fs::read_dir(staging)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0);
}

#[test]
fn download_size_limit_is_enforced() {
    let mut probe =
        parse_probe_output("status=ok\nuser=u\nuid=1\nkind=dir\nsize_kib=1\ntar=gnu\nbase64=yes\n")
            .unwrap();
    let limits = SyncLimits::default();
    assert!(ensure_download_size(&probe, limits).is_ok());

    probe.size_kib = Some(limits.max_download_kib);
    assert!(ensure_download_size(&probe, limits).is_ok());

    probe.size_kib = Some(limits.max_download_kib + 1);
    assert!(matches!(
        ensure_download_size(&probe, limits),
        Err(WarpSyncError::TooLarge { .. })
    ));

    probe.size_kib = None;
    assert!(ensure_download_size(&probe, limits).is_ok());
}

#[test]
fn download_size_limit_follows_the_configured_limit() {
    let mut probe =
        parse_probe_output("status=ok\nuser=u\nuid=1\nkind=dir\nsize_kib=2048\ntar=gnu\nbase64=yes\n")
            .unwrap();

    assert!(ensure_download_size(&probe, SyncLimits::from_mib(2, 4)).is_ok());
    assert!(matches!(
        ensure_download_size(&probe, SyncLimits::from_mib(1, 4)),
        Err(WarpSyncError::TooLarge { .. })
    ));
    probe.size_kib = Some(2049);
    assert!(matches!(
        ensure_download_size(&probe, SyncLimits::from_mib(2, 4)),
        Err(WarpSyncError::TooLarge { .. })
    ));
}

#[test]
fn upload_without_a_download_is_not_mirrored() {
    let env = Env::new();

    let result = env.upload();

    assert!(matches!(result, Err(WarpSyncError::NotMirrored(_))));
}

#[test]
fn upload_replaces_remote_files_backs_them_up_and_cleans_up() {
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited a").unwrap();
    fs::write(env.local("d.conf"), "new d").unwrap();
    fs::remove_file(env.local("b.conf")).unwrap();

    let outcome = env.upload().unwrap();

    assert_eq!(
        fs::read_to_string(env.remote("a.conf")).unwrap(),
        "edited a"
    );
    assert_eq!(fs::read_to_string(env.remote("d.conf")).unwrap(), "new d");
    assert_eq!(
        fs::read_to_string(env.remote("b.conf")).unwrap(),
        "remote b"
    );
    assert_eq!(
        fs::read_to_string(env.remote("sub/c.conf")).unwrap(),
        "remote c"
    );

    let backup = outcome
        .backup_path
        .expect("the target existed, so it is backed up");
    assert!(Path::new(&backup).starts_with(env.dir.path().join("home/.warp-sync/backups")));
    assert!(Path::new(&backup).is_file());
    assert!(env.leftover_scratch_dirs().is_empty());
    assert_eq!((outcome.files, outcome.dirs), (3, 2));

    let manifest = env.manifest();
    let edited = manifest
        .entry(&format!("{}/a.conf", env.remote_path))
        .unwrap();
    assert_eq!(
        edited.sha256.as_deref(),
        Some(sha256_hex("edited a").as_str())
    );
    assert!(
        manifest
            .entry(&format!("{}/d.conf", env.remote_path))
            .is_some()
    );
    assert!(
        manifest
            .entry(&format!("{}/b.conf", env.remote_path))
            .is_some()
    );
}

#[test]
fn upload_of_an_untouched_remote_reports_no_conflicts() {
    if !has_hash_tool() {
        return;
    }
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited a").unwrap();

    let prepared = env.prepare();

    assert!(conflicts_of(&prepared).is_empty());
}

#[test]
fn upload_reports_what_changed_on_the_remote_host_since_the_download() {
    if !has_hash_tool() {
        return;
    }
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited a").unwrap();
    fs::write(env.local("d.conf"), "new d").unwrap();
    fs::write(env.remote("a.conf"), "someone else's edit").unwrap();
    fs::remove_file(env.remote("sub/c.conf")).unwrap();
    fs::write(env.remote("d.conf"), "someone else's d").unwrap();

    let conflicts = conflicts_of(&env.prepare());

    let remote = |relative: &str| env.remote(relative).to_str().unwrap().to_owned();
    assert_eq!(conflicts.changed, vec![remote("a.conf")]);
    assert_eq!(conflicts.missing, vec![remote("sub/c.conf")]);
    assert_eq!(conflicts.already_exist, vec![remote("d.conf")]);
}

#[test]
fn upload_without_a_hash_tool_on_the_remote_host_is_unchecked() {
    let env = Env::new();
    env.download_done();

    let prepared = block_on(prepare_upload(
        &WithoutHashTool(&env.shell),
        &env.upload_request(),
    ))
    .unwrap();

    assert_eq!(prepared.remote_check, RemoteCheck::Unavailable);
}

#[test]
fn a_failing_hash_command_does_not_block_the_upload() {
    struct FailsToHash<'a>(&'a LocalSh);

    #[async_trait]
    impl RemoteShell for FailsToHash<'_> {
        async fn run(&self, command: &str) -> Result<Vec<u8>, WarpSyncError> {
            use base64::Engine as _;
            let script = command
                .split_whitespace()
                .nth(2)
                .and_then(|encoded| base64::engine::general_purpose::STANDARD.decode(encoded).ok())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            if script.contains("-exec $H") {
                return Err(WarpSyncError::RemoteCommandFailed {
                    exit_code: Some(127),
                    message: "not found".to_owned(),
                });
            }
            self.0.run(command).await
        }
    }
    let env = Env::new();
    env.download_done();

    let prepared = block_on(prepare_upload(
        &FailsToHash(&env.shell),
        &env.upload_request(),
    ))
    .unwrap();

    assert_eq!(prepared.remote_check, RemoteCheck::Unavailable);
}

#[test]
fn upload_backup_contains_the_previous_contents() {
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited a").unwrap();

    let outcome = env.upload().unwrap();

    let backup = outcome.backup_path.unwrap();
    let listing = Command::new("tar")
        .args(["-xzOf", &backup, "./conf/a.conf"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&listing.stdout), "remote a");
}

#[test]
fn failed_upload_removes_the_remote_scratch_directory() {
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited a").unwrap();
    let prepared = block_on(prepare_upload(&env.shell, &env.upload_request())).unwrap();
    // Calls: begin, one chunk, commit (fails), then the cleanup.
    env.shell.fail_on_call_from_now(3);

    let result = block_on(execute_upload(&env.shell, &prepared));

    assert!(matches!(result, Err(WarpSyncError::Executor(_))));
    assert!(env.leftover_scratch_dirs().is_empty());
    assert_eq!(
        fs::read_to_string(env.remote("a.conf")).unwrap(),
        "remote a"
    );
}

#[cfg(unix)]
#[test]
fn upload_that_cannot_extract_reports_the_failure_and_cleans_up() {
    use std::os::unix::fs::PermissionsExt;

    if running_as_root() {
        return;
    }
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited a").unwrap();
    let prepared = block_on(prepare_upload(&env.shell, &env.upload_request())).unwrap();
    let read_only = fs::Permissions::from_mode(0o555);
    fs::set_permissions(env.remote(""), read_only).unwrap();

    let result = block_on(execute_upload(&env.shell, &prepared));

    fs::set_permissions(env.remote(""), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        result,
        Err(WarpSyncError::RemoteCommandFailed { .. })
    ));
    assert!(env.leftover_scratch_dirs().is_empty());
    assert_eq!(
        fs::read_to_string(env.remote("a.conf")).unwrap(),
        "remote a"
    );
}

#[test]
fn upload_needs_base64_on_the_remote_host() {
    struct NoBase64;

    #[async_trait]
    impl RemoteShell for NoBase64 {
        async fn run(&self, _command: &str) -> Result<Vec<u8>, WarpSyncError> {
            Ok(b"status=ok\nuser=u\nuid=1\nkind=dir\nsize_kib=1\ntar=gnu\nbase64=no\n".to_vec())
        }
    }
    let env = Env::new();
    env.download_done();

    let result = block_on(prepare_upload(&NoBase64, &env.upload_request()));

    assert!(matches!(result, Err(WarpSyncError::MissingTool("base64"))));
}

#[test]
fn swap_restores_the_previous_copy_when_the_move_fails() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("mirror/conf");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("keep"), "precious").unwrap();

    let result = swap_into_place(
        &dir.path().join("does-not-exist"),
        &target,
        &dir.path().join("previous"),
        &dir.path().join("recovered"),
    );

    assert!(matches!(result, Err(WarpSyncError::LocalIo(_))));
    assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "precious");
}

#[test]
fn undoing_a_swap_puts_the_previous_copy_back() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("mirror/conf");
    let new = dir.path().join("new/conf");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("file"), "old").unwrap();
    fs::create_dir_all(&new).unwrap();
    fs::write(new.join("file"), "new").unwrap();

    let undo = swap_into_place(
        &new,
        &target,
        &dir.path().join("previous"),
        &dir.path().join("recovered"),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(target.join("file")).unwrap(), "new");

    undo.undo();

    assert_eq!(fs::read_to_string(target.join("file")).unwrap(), "old");
    assert_eq!(fs::read_to_string(new.join("file")).unwrap(), "new");
}

#[test]
fn undoing_a_first_time_swap_removes_the_new_copy_from_the_mirror() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("mirror/conf");
    let new = dir.path().join("new/conf");
    fs::create_dir_all(&new).unwrap();
    fs::write(new.join("file"), "new").unwrap();

    let undo = swap_into_place(
        &new,
        &target,
        &dir.path().join("previous"),
        &dir.path().join("recovered"),
    )
    .unwrap();
    undo.undo();

    assert!(!target.exists());
    assert!(new.join("file").is_file());
}

#[cfg(unix)]
#[test]
fn a_manifest_that_cannot_be_saved_leaves_the_previous_mirror_in_place() {
    use std::os::unix::fs::PermissionsExt;

    if running_as_root() {
        return;
    }
    let env = Env::new();
    env.download_done();
    fs::write(env.remote("a.conf"), "remote a v2").unwrap();
    let state_dir = env.mirror_root().join(".warp-sync");
    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o555)).unwrap();

    let result = env.download(false);

    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(result, Err(WarpSyncError::Manifest(_))),
        "{result:?}"
    );
    assert_eq!(fs::read_to_string(env.local("a.conf")).unwrap(), "remote a");
    let a = env
        .manifest()
        .entry(&format!("{}/a.conf", env.remote_path))
        .cloned()
        .unwrap();
    assert_eq!(a.sha256.as_deref(), Some(sha256_hex("remote a").as_str()));
}

#[test]
fn edits_made_while_a_download_runs_are_not_overwritten() {
    let env = Env::new();
    env.download_done();
    let (parent, name) = split_parent_name(&env.remote_path);
    let tgz = block_on(
        env.shell
            .run(&wrap_for_any_shell(&download_script(&parent, &name))),
    )
    .unwrap();
    let probe = block_on(probe(&env.shell, &env.remote_path)).unwrap();
    fs::write(env.local("a.conf"), "typed during the download").unwrap();
    let staging = staging_dir(&env.mirror_root());

    let result = apply_download(&tgz, &env.download_request(false), &probe, &staging);

    let Ok(DownloadResult::NeedsConfirmation { modified_files }) = result else {
        panic!("expected a confirmation request, got {result:?}");
    };
    assert_eq!(modified_files, [format!("{}/a.conf", env.remote_path)]);
    assert_eq!(
        fs::read_to_string(env.local("a.conf")).unwrap(),
        "typed during the download"
    );
}

#[test]
fn a_remote_entry_named_like_the_parking_directory_does_not_clobber_the_download() {
    let env = Env::new();
    let request = DownloadRequest {
        remote_path: format!("{}/{PREVIOUS_COPY_DIR}", env.remote_path),
        ..env.download_request(false)
    };
    fs::write(env.remote(PREVIOUS_COPY_DIR), "first").unwrap();
    assert!(matches!(
        block_on(download(&env.shell, &request)),
        Ok(DownloadResult::Done(_))
    ));
    fs::write(env.remote(PREVIOUS_COPY_DIR), "second").unwrap();

    let result = block_on(download(&env.shell, &request));

    assert!(matches!(result, Ok(DownloadResult::Done(_))), "{result:?}");
    assert_eq!(
        fs::read_to_string(env.local(PREVIOUS_COPY_DIR)).unwrap(),
        "second"
    );
}

#[test]
fn oversized_output_is_rejected_even_when_du_said_otherwise() {
    let limits = SyncLimits::default();
    let limit_bytes = (limits.max_download_kib * BYTES_PER_KIB) as usize;
    assert!(ensure_received_size(0, limits).is_ok());
    assert!(ensure_received_size(limit_bytes, limits).is_ok());
    assert!(matches!(
        ensure_received_size(limit_bytes + 1, limits),
        Err(WarpSyncError::TooLarge { .. })
    ));
}

#[cfg(unix)]
#[test]
fn the_mirror_root_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::new();

    env.download_done();

    let mode = fs::metadata(env.mirror_root())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn backup_names_use_only_safe_characters() {
    let name = backup_name("prod-1", "/etc/my app/it's$(x)", 1_727_000_000);

    assert!(
        name.starts_with("prod-1_etc_my_app_it_s__x_-1727000000-"),
        "{name}"
    );
    assert!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    );
}

#[test]
fn backup_names_do_not_repeat_within_a_second() {
    assert_ne!(
        backup_name("h", "/etc/a", 1_727_000_000),
        backup_name("h", "/etc/a", 1_727_000_000)
    );
}

#[test]
fn backup_names_are_bounded() {
    let long_path = format!("/{}", "a".repeat(1000));

    let name = backup_name("h", &long_path, 1);

    assert!(name.len() <= MAX_BACKUP_STEM_CHARS + "-1-".len() + BACKUP_NONCE_CHARS);
    assert!(name.contains("-1-"));
}

#[test]
fn backup_path_is_read_from_the_commit_output() {
    assert_eq!(
        parse_backup_path("noise\nbackup=/root/.warp-sync/backups/x.tgz\r\n"),
        Some("/root/.warp-sync/backups/x.tgz".to_owned())
    );
    assert_eq!(parse_backup_path(""), None);
}

const MACHINE_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MACHINE_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn save_manifest_owned_by(mirror_root: &Path, host_key: &str, machine_id: Option<&str>) {
    let mut manifest = Manifest::new(host_key);
    manifest.set_machine_id(machine_id.map(str::to_owned));
    manifest
        .save_atomic(&manifest_path(mirror_root, host_key))
        .unwrap();
}

#[test]
fn a_mirror_without_a_known_owner_keeps_the_plain_name() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(
        resolve_host_key(dir.path(), "h", Some(MACHINE_A)).unwrap(),
        "h"
    );
    assert_eq!(resolve_host_key(dir.path(), "h", None).unwrap(), "h");

    save_manifest_owned_by(dir.path(), "h", None);
    assert_eq!(
        resolve_host_key(dir.path(), "h", Some(MACHINE_A)).unwrap(),
        "h"
    );
}

#[test]
fn the_plain_name_belongs_to_the_machine_that_first_used_it() {
    let dir = tempfile::tempdir().unwrap();
    save_manifest_owned_by(dir.path(), "h", Some(MACHINE_A));

    assert_eq!(
        resolve_host_key(dir.path(), "h", Some(MACHINE_A)).unwrap(),
        "h"
    );
    let other = resolve_host_key(dir.path(), "h", Some(MACHINE_B)).unwrap();
    assert_eq!(other, machine_host_key("h", MACHINE_B));
    assert_ne!(other, "h");
}

#[test]
fn a_host_that_reports_no_machine_id_uses_the_plain_name() {
    let dir = tempfile::tempdir().unwrap();
    save_manifest_owned_by(dir.path(), "h", Some(MACHINE_A));

    assert_eq!(resolve_host_key(dir.path(), "h", None).unwrap(), "h");
}

#[test]
fn a_name_owned_by_two_other_machines_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    save_manifest_owned_by(dir.path(), "h", Some(MACHINE_A));
    let suffixed = machine_host_key("h", MACHINE_B);
    save_manifest_owned_by(dir.path(), &suffixed, Some(MACHINE_A));

    let result = resolve_host_key(dir.path(), "h", Some(MACHINE_B));

    assert!(matches!(result, Err(WarpSyncError::Manifest(_))));
}

#[test]
fn two_machines_with_the_same_hostname_get_separate_mirrors() {
    let env = Env::new();
    let machine_a = AsMachine {
        shell: &env.shell,
        machine_id: MACHINE_A,
    };
    let machine_b = AsMachine {
        shell: &env.shell,
        machine_id: MACHINE_B,
    };

    block_on(download(&machine_a, &env.download_request(false))).unwrap();
    fs::write(env.remote("a.conf"), "remote a on B").unwrap();
    block_on(download(&machine_b, &env.download_request(false))).unwrap();

    let dir_b = machine_host_key(HOST_KEY, MACHINE_B);
    let mirror_a = local_path_for(&env.mirror_root(), HOST_KEY, &env.remote_path);
    let mirror_b = local_path_for(&env.mirror_root(), &dir_b, &env.remote_path);
    assert_eq!(
        fs::read_to_string(mirror_a.join("a.conf")).unwrap(),
        "remote a"
    );
    assert_eq!(
        fs::read_to_string(mirror_b.join("a.conf")).unwrap(),
        "remote a on B"
    );
    let owner = |key: &str| {
        load_manifest(&env.mirror_root(), key)
            .unwrap()
            .machine_id()
            .map(str::to_owned)
    };
    assert_eq!(owner(HOST_KEY).as_deref(), Some(MACHINE_A));
    assert_eq!(owner(&dir_b).as_deref(), Some(MACHINE_B));
}

#[test]
fn a_mirror_downloaded_from_one_machine_cannot_be_uploaded_to_another() {
    let env = Env::new();
    let machine_a = AsMachine {
        shell: &env.shell,
        machine_id: MACHINE_A,
    };
    let machine_b = AsMachine {
        shell: &env.shell,
        machine_id: MACHINE_B,
    };
    block_on(download(&machine_a, &env.download_request(false))).unwrap();
    fs::write(env.local("a.conf"), "edited for A").unwrap();

    let onto_b = block_on(prepare_upload(&machine_b, &env.upload_request()));
    let onto_a = block_on(prepare_upload(&machine_a, &env.upload_request()));

    assert!(matches!(onto_b, Err(WarpSyncError::NotMirrored(_))));
    assert_eq!(onto_a.unwrap().host_key, HOST_KEY);
}
