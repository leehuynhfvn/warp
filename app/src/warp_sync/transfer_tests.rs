use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use command::blocking::Command;
use futures::executor::block_on;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::super::diff::FileChange;
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
            .and_then(|encoded| {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .ok()
            })
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
            expected_host_key: None,
            mirror_root: self.mirror_root(),
            limits: SyncLimits::default(),
            allow_overwrite_local_changes,
        }
    }

    fn upload_request(&self) -> UploadRequest {
        UploadRequest {
            remote_path: self.remote_path.clone(),
            host_key: HOST_KEY.to_owned(),
            expected_host_key: None,
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

    fn compare_request(&self) -> CompareRequest {
        CompareRequest {
            remote_path: self.remote_path.clone(),
            hostname: "prod-1".to_owned(),
            host_key: HOST_KEY.to_owned(),
            expected_host_key: None,
            mirror_root: self.mirror_root(),
            limits: SyncLimits::default(),
        }
    }

    fn compare(&self) -> Result<CompareOutcome, WarpSyncError> {
        block_on(compare(&self.shell, &self.compare_request()))
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

/// What the Git baseline at the root of the host's mirror has for `relative` under the synced
/// path, or `None` when it has nothing.
fn baseline_contents(env: &Env, relative: &str) -> Option<String> {
    let host_dir = env.mirror_root().join(HOST_KEY);
    let path = format!("{}/{relative}", env.remote_path.trim_start_matches('/'));
    let output = Command::new("git")
        .arg("-C")
        .arg(&host_dir)
        .args(["show", &format!("HEAD:{path}")])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap())
}

#[test]
fn download_and_upload_keep_the_git_baseline_at_what_the_server_has() {
    let env = Env::new();

    let downloaded = env.download_done();
    assert_eq!(downloaded.host_dir, env.mirror_root().join(HOST_KEY));
    assert_eq!(downloaded.baseline_warning, None);
    assert_eq!(
        baseline_contents(&env, "a.conf").as_deref(),
        Some("remote a")
    );

    fs::write(env.local("a.conf"), "edited a").unwrap();
    fs::remove_file(env.local("b.conf")).unwrap();
    let uploaded = env.upload().unwrap();

    assert_eq!(uploaded.baseline_warning, None);
    assert_eq!(
        baseline_contents(&env, "a.conf").as_deref(),
        Some("edited a")
    );
    // Still on the server, since uploading does not delete.
    assert_eq!(
        baseline_contents(&env, "b.conf").as_deref(),
        Some("remote b")
    );
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
    let mut probe = parse_probe_output(
        "status=ok\nuser=u\nuid=1\nkind=dir\nsize_kib=2048\ntar=gnu\nbase64=yes\n",
    )
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
                .and_then(|encoded| {
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .ok()
                })
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
fn compare_of_an_unchanged_mirror_finds_nothing_and_writes_no_diff() {
    let env = Env::new();
    env.download_done();

    let outcome = env.compare().unwrap();

    assert!(outcome.differences.is_empty());
    assert_eq!(outcome.identical_files, 3);
    assert_eq!(outcome.diff_path, None);
}

#[test]
fn compare_reports_changes_on_both_sides_and_writes_a_diff() {
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited locally").unwrap();
    fs::write(env.remote("b.conf"), "edited on the server").unwrap();
    fs::write(env.local("new.conf"), "brand new").unwrap();
    fs::remove_file(env.local("sub/c.conf")).unwrap();

    let outcome = env.compare().unwrap();

    let changes: Vec<(String, FileChange)> = outcome
        .differences
        .iter()
        .map(|difference| {
            let relative = difference
                .remote_path
                .strip_prefix(&format!("{}/", env.remote_path))
                .unwrap()
                .to_owned();
            (relative, difference.change)
        })
        .collect();
    assert_eq!(
        changes,
        vec![
            ("a.conf".to_owned(), FileChange::ChangedLocally),
            ("b.conf".to_owned(), FileChange::ChangedOnServer),
            ("new.conf".to_owned(), FileChange::NewLocally),
            ("sub/c.conf".to_owned(), FileChange::DeletedLocally),
        ]
    );
    let diff_path = outcome.diff_path.expect("there are differences");
    assert!(diff_path.starts_with(env.mirror_root().join(".warp-sync/diffs")));
    let diff = fs::read_to_string(&diff_path).unwrap();
    assert!(diff.starts_with("# Warp Sync: prod-1:"), "{diff}");
    assert!(
        diff.contains("-remote a\n\\ No newline at end of file\n+edited locally"),
        "{diff}"
    );
    assert!(
        diff.contains("-edited on the server") || diff.contains("+remote b"),
        "{diff}"
    );
}

#[cfg(unix)]
#[test]
fn the_comparison_file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited").unwrap();

    let diff_path = env.compare().unwrap().diff_path.unwrap();

    let mode = fs::metadata(diff_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

/// Where the latest comparison keeps the server's copy of `relative` under the synced path.
fn server_copy(env: &Env, outcome: &CompareOutcome, relative: &str) -> PathBuf {
    outcome
        .server_copy_dir
        .join(env.remote_path.trim_start_matches('/'))
        .join(relative)
}

#[test]
fn compare_keeps_a_read_only_copy_of_the_server_side() {
    let env = Env::new();
    env.download_done();
    fs::write(env.remote("a.conf"), "edited on the server").unwrap();

    let outcome = env.compare().unwrap();

    assert_eq!(outcome.host_dir, env.mirror_root().join(HOST_KEY));
    let copy = server_copy(&env, &outcome, "a.conf");
    assert_eq!(fs::read_to_string(&copy).unwrap(), "edited on the server");
    assert_eq!(
        fs::read_to_string(server_copy(&env, &outcome, "sub/c.conf")).unwrap(),
        "remote c"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&copy).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o400);
    }
}

#[test]
fn a_later_comparison_replaces_the_server_copy() {
    let env = Env::new();
    env.download_done();
    fs::write(env.remote("a.conf"), "first").unwrap();
    env.compare().unwrap();
    fs::write(env.remote("a.conf"), "second").unwrap();
    fs::remove_file(env.remote("b.conf")).unwrap();

    let outcome = env.compare().unwrap();

    assert_eq!(
        fs::read_to_string(server_copy(&env, &outcome, "a.conf")).unwrap(),
        "second"
    );
    assert!(!server_copy(&env, &outcome, "b.conf").exists());
}

#[test]
fn a_comparison_without_differences_removes_the_previous_one() {
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited").unwrap();
    let earlier = env.compare().unwrap();
    let diff_path = earlier.diff_path.clone().unwrap();
    fs::write(env.local("a.conf"), "remote a").unwrap();

    let outcome = env.compare().unwrap();

    assert_eq!(outcome.diff_path, None);
    assert!(!diff_path.exists());
    assert!(!server_copy(&env, &outcome, "").exists());
}

#[test]
fn compare_leaves_the_mirror_and_manifest_alone() {
    let env = Env::new();
    env.download_done();
    fs::write(env.local("a.conf"), "edited locally").unwrap();
    fs::write(env.remote("a.conf"), "edited on the server").unwrap();
    let manifest_before = env.manifest();

    env.compare().unwrap();

    assert_eq!(
        fs::read_to_string(env.local("a.conf")).unwrap(),
        "edited locally"
    );
    assert_eq!(env.manifest(), manifest_before);
    let staging = env.mirror_root().join(".warp-sync/staging");
    let leftovers = fs::read_dir(staging).map(|dir| dir.count()).unwrap_or(0);
    assert_eq!(leftovers, 0);
}

#[test]
fn compare_without_a_download_is_not_mirrored() {
    let env = Env::new();

    let result = env.compare();

    assert!(matches!(result, Err(WarpSyncError::NotMirrored(_))));
}

#[test]
fn compare_of_a_missing_path_is_not_found() {
    let env = Env::new();
    env.download_done();
    fs::remove_dir_all(env.remote("")).unwrap();

    let result = env.compare();

    assert!(matches!(result, Err(WarpSyncError::NotFound(_))));
}

#[test]
fn a_hash_command_that_times_out_does_not_block_the_upload() {
    struct TimesOut<'a>(&'a LocalSh);

    #[async_trait]
    impl RemoteShell for TimesOut<'_> {
        async fn run(&self, command: &str) -> Result<Vec<u8>, WarpSyncError> {
            use base64::Engine as _;
            let script = command
                .split_whitespace()
                .nth(2)
                .and_then(|encoded| {
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .ok()
                })
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            if script.contains("-exec $H") {
                return Err(WarpSyncError::Timeout);
            }
            self.0.run(command).await
        }
    }
    let env = Env::new();
    env.download_done();

    let prepared = block_on(prepare_upload(&TimesOut(&env.shell), &env.upload_request())).unwrap();

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
fn a_host_that_reports_no_machine_id_cannot_use_a_mirror_that_belongs_to_a_machine() {
    let dir = tempfile::tempdir().unwrap();
    save_manifest_owned_by(dir.path(), "h", Some(MACHINE_A));

    let result = resolve_host_key(dir.path(), "h", None);

    assert!(
        matches!(&result, Err(WarpSyncError::Manifest(message)) if message.contains("did not report a machine id")),
        "{result:?}"
    );
}

#[test]
fn a_host_that_reports_no_machine_id_can_use_a_mirror_nobody_owns() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(resolve_host_key(dir.path(), "h", None).unwrap(), "h");

    save_manifest_owned_by(dir.path(), "h", None);
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

#[test]
fn a_download_for_another_mirror_folder_is_refused_before_anything_is_written() {
    let env = Env::new();
    let request = DownloadRequest {
        expected_host_key: Some("other-host".to_owned()),
        ..env.download_request(false)
    };

    let result = block_on(download(&env.shell, &request));

    assert!(
        matches!(&result, Err(WarpSyncError::Manifest(message)) if message.contains("another machine")),
        "{result:?}"
    );
    assert!(!env.mirror_root().join(HOST_KEY).exists());
}

#[test]
fn a_download_for_the_folder_the_host_resolves_to_goes_ahead() {
    let env = Env::new();
    let request = DownloadRequest {
        expected_host_key: Some(HOST_KEY.to_owned()),
        ..env.download_request(false)
    };

    let result = block_on(download(&env.shell, &request));

    assert!(matches!(result, Ok(DownloadResult::Done(_))), "{result:?}");
}

#[test]
fn an_upload_for_another_mirror_folder_is_refused_before_it_is_packed() {
    let env = Env::new();
    env.download_done();
    let request = UploadRequest {
        expected_host_key: Some("other-host".to_owned()),
        ..env.upload_request()
    };

    let result = block_on(prepare_upload(&env.shell, &request));

    assert!(
        matches!(&result, Err(WarpSyncError::Manifest(message)) if message.contains("another machine")),
        "{:?}",
        result.as_ref().err()
    );
}

#[test]
fn a_comparison_for_another_mirror_folder_is_refused() {
    let env = Env::new();
    env.download_done();
    let request = CompareRequest {
        expected_host_key: Some("other-host".to_owned()),
        ..env.compare_request()
    };

    let result = block_on(compare(&env.shell, &request));

    assert!(
        matches!(&result, Err(WarpSyncError::Manifest(message)) if message.contains("another machine")),
        "{:?}",
        result.as_ref().err()
    );
}

#[test]
fn an_upload_is_refused_when_the_session_no_longer_reaches_the_prepared_machine() {
    let env = Env::new();
    env.download_done();
    std::fs::write(env.local("a.conf"), "local edit").unwrap();
    let prepared = env.prepare();
    let other_machine = AsMachine {
        shell: &env.shell,
        machine_id: MACHINE_B,
    };

    let result = block_on(execute_upload(&other_machine, &prepared));

    assert!(
        matches!(&result, Err(WarpSyncError::Manifest(message)) if message.contains("no longer reaches")),
        "{result:?}"
    );
    assert_eq!(
        fs::read_to_string(env.remote("a.conf")).unwrap(),
        "remote a",
        "nothing may be written on the other machine"
    );
}

// Uploading paths that do not exist on the server yet.

impl Env {
    fn request_for_new(&self, relative: &str) -> UploadRequest {
        UploadRequest {
            remote_path: format!("{}/{relative}", self.remote_path),
            ..self.upload_request()
        }
    }

    fn prepare_new(&self, relative: &str) -> Result<PreparedUpload, WarpSyncError> {
        block_on(prepare_upload(&self.shell, &self.request_for_new(relative)))
    }

    fn upload_new(&self, relative: &str) -> Result<UploadOutcome, WarpSyncError> {
        let prepared = self.prepare_new(relative)?;
        block_on(execute_upload(&self.shell, &prepared))
    }

    /// A downloaded mirror with a new local file `relative` (and its parents).
    fn with_new_local_file(relative: &str, contents: &str) -> Self {
        let env = Self::new();
        env.download_done();
        let local = env.local(relative);
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        fs::write(local, contents).unwrap();
        env
    }
}

fn exit_code_of(result: &Result<UploadOutcome, WarpSyncError>) -> Option<i32> {
    match result {
        Err(WarpSyncError::RemoteCommandFailed { exit_code, .. }) => *exit_code,
        Ok(_) | Err(_) => None,
    }
}

#[test]
fn a_new_file_in_a_synced_directory_uploads_without_touching_its_siblings() {
    let env = Env::with_new_local_file("fresh.conf", "fresh");
    fs::write(env.local("a.conf"), "edited a, not to be uploaded").unwrap();

    let outcome = env.upload_new("fresh.conf").unwrap();

    assert_eq!(
        fs::read_to_string(env.remote("fresh.conf")).unwrap(),
        "fresh"
    );
    assert_eq!(
        fs::read_to_string(env.remote("a.conf")).unwrap(),
        "remote a"
    );
    assert_eq!((outcome.files, outcome.dirs), (1, 0));
    assert_eq!(outcome.backup_path, None, "nothing was replaced");
    assert!(env.leftover_scratch_dirs().is_empty());
    assert!(
        !env.dir.path().join("home/.warp-sync").exists(),
        "there is nothing to back up"
    );
}

#[test]
fn a_new_directory_tree_is_created_with_every_level_in_between() {
    let env = Env::with_new_local_file("one/two/three/deep.conf", "deep");
    fs::write(env.local("one/top.conf"), "top").unwrap();

    let outcome = env.upload_new("one").unwrap();

    assert_eq!(
        fs::read_to_string(env.remote("one/two/three/deep.conf")).unwrap(),
        "deep"
    );
    assert_eq!(
        fs::read_to_string(env.remote("one/top.conf")).unwrap(),
        "top"
    );
    assert_eq!((outcome.files, outcome.dirs), (2, 3));
}

#[test]
fn a_new_file_several_levels_below_a_synced_directory_creates_the_levels_between() {
    let env = Env::with_new_local_file("one/two/only.conf", "only");
    fs::write(env.local("one/sibling.conf"), "not uploaded").unwrap();

    env.upload_new("one/two/only.conf").unwrap();

    assert_eq!(
        fs::read_to_string(env.remote("one/two/only.conf")).unwrap(),
        "only"
    );
    assert!(
        !env.remote("one/sibling.conf").exists(),
        "only the requested path is uploaded"
    );
}

#[test]
fn the_manifest_records_the_new_path_and_the_levels_created_for_it() {
    let env = Env::with_new_local_file("one/two/only.conf", "only");

    env.upload_new("one/two/only.conf").unwrap();

    let manifest = env.manifest();
    for relative in ["one", "one/two", "one/two/only.conf"] {
        assert!(
            manifest
                .entries_under(&format!("{}/{relative}", env.remote_path))
                .contains_key(&format!("{}/{relative}", env.remote_path)),
            "{relative}"
        );
    }
    let file = manifest
        .entry(&format!("{}/one/two/only.conf", env.remote_path))
        .unwrap();
    assert_eq!(file.sha256.as_deref(), Some(sha256_hex("only").as_str()));
}

#[test]
fn a_path_uploaded_before_is_replaced_and_backed_up_the_next_time() {
    let env = Env::with_new_local_file("one/only.conf", "first");
    env.upload_new("one/only.conf").unwrap();
    fs::write(env.local("one/only.conf"), "second").unwrap();

    let outcome = env.upload_new("one/only.conf").unwrap();

    assert_eq!(
        fs::read_to_string(env.remote("one/only.conf")).unwrap(),
        "second"
    );
    assert!(outcome.backup_path.is_some());
}

#[test]
fn the_summary_lists_what_will_be_created_including_the_levels_in_between() {
    let env = Env::with_new_local_file("one/two/only.conf", "only");

    let prepared = env.prepare_new("one/two/only.conf").unwrap();

    let prefix = &env.remote_path;
    assert_eq!(
        prepared.archive.new_files,
        [
            format!("{prefix}/one"),
            format!("{prefix}/one/two"),
            format!("{prefix}/one/two/only.conf"),
        ]
    );
    assert_eq!(prepared.remote_check, RemoteCheck::Checked(Default::default()));
}

#[cfg(unix)]
#[test]
fn a_new_file_keeps_the_permission_bits_it_has_locally() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::with_new_local_file("secret.conf", "s");
    fs::set_permissions(env.local("secret.conf"), fs::Permissions::from_mode(0o600)).unwrap();

    env.upload_new("secret.conf").unwrap();

    let mode = fs::metadata(env.remote("secret.conf"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn a_new_file_with_setuid_bits_is_refused_and_nothing_is_created() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::with_new_local_file("one/tool.sh", "x");
    fs::set_permissions(env.local("one/tool.sh"), fs::Permissions::from_mode(0o4755)).unwrap();

    let result = env.prepare_new("one");

    assert!(matches!(result, Err(WarpSyncError::SpecialMode(_))));
    assert!(!env.remote("one").exists());
}

#[cfg(unix)]
#[test]
fn the_levels_created_in_between_are_world_readable_directories() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::with_new_local_file("one/two/only.conf", "only");

    env.upload_new("one/two/only.conf").unwrap();

    let mode = fs::metadata(env.remote("one")).unwrap().permissions().mode();
    assert_eq!(mode & 0o7777, 0o755);
}

#[test]
fn a_path_that_appears_before_the_confirmation_is_not_overwritten() {
    let env = Env::with_new_local_file("fresh.conf", "mine");
    let prepared = env.prepare_new("fresh.conf").unwrap();
    fs::write(env.remote("fresh.conf"), "theirs").unwrap();

    let result = block_on(execute_upload(&env.shell, &prepared));

    assert_eq!(
        exit_code_of(&result),
        Some(super::super::remote_script::EXIT_TARGET_EXISTS),
        "{result:?}"
    );
    assert_eq!(
        fs::read_to_string(env.remote("fresh.conf")).unwrap(),
        "theirs"
    );
    assert!(env.leftover_scratch_dirs().is_empty());
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_that_appears_at_the_path_is_not_written_through() {
    let env = Env::with_new_local_file("fresh.conf", "mine");
    let prepared = env.prepare_new("fresh.conf").unwrap();
    let outside = env.dir.path().join("outside");
    std::os::unix::fs::symlink(&outside, env.remote("fresh.conf")).unwrap();

    let result = block_on(execute_upload(&env.shell, &prepared));

    assert_eq!(
        exit_code_of(&result),
        Some(super::super::remote_script::EXIT_TARGET_EXISTS),
        "{result:?}"
    );
    assert!(!outside.exists());
}

#[cfg(unix)]
#[test]
fn a_level_in_between_that_appears_before_the_confirmation_is_left_alone() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::with_new_local_file("one/two/only.conf", "only");
    let prepared = env.prepare_new("one/two/only.conf").unwrap();
    fs::create_dir(env.remote("one")).unwrap();
    fs::set_permissions(env.remote("one"), fs::Permissions::from_mode(0o700)).unwrap();

    let result = block_on(execute_upload(&env.shell, &prepared));

    assert_eq!(
        exit_code_of(&result),
        Some(super::super::remote_script::EXIT_TARGET_EXISTS),
        "{result:?}"
    );
    let mode = fs::metadata(env.remote("one")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "the directory that was there is untouched");
    assert!(!env.remote("one/two").exists());
}

#[cfg(unix)]
#[test]
fn a_synced_directory_that_became_a_symlink_is_not_written_into() {
    let env = Env::with_new_local_file("fresh.conf", "mine");
    let prepared = env.prepare_new("fresh.conf").unwrap();
    let elsewhere = env.dir.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let anchor = Path::new(&env.remote_path);
    fs::rename(anchor, env.dir.path().join("remote/moved")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, anchor).unwrap();

    let result = block_on(execute_upload(&env.shell, &prepared));

    assert_eq!(
        exit_code_of(&result),
        Some(super::super::remote_script::EXIT_ANCHOR_UNSAFE),
        "{result:?}"
    );
    assert!(!elsewhere.join("fresh.conf").exists());
    assert!(env.leftover_scratch_dirs().is_empty());
}

#[test]
fn a_new_path_needs_a_synced_ancestor_that_still_exists_on_the_server() {
    let env = Env::with_new_local_file("sub/one/only.conf", "only");
    fs::remove_dir_all(env.remote("sub")).unwrap();

    let result = env.prepare_new("sub/one/only.conf");

    assert!(
        matches!(&result, Err(WarpSyncError::NotFound(path)) if path.ends_with("/conf/sub")),
        "{:?}",
        result.as_ref().err()
    );
}

#[test]
fn a_synced_ancestor_that_is_a_file_on_the_server_is_refused() {
    let env = Env::with_new_local_file("sub/one/only.conf", "only");
    fs::remove_dir_all(env.remote("sub")).unwrap();
    fs::write(env.remote("sub"), "now a file").unwrap();

    let result = env.prepare_new("sub/one/only.conf");

    assert!(
        matches!(&result, Err(WarpSyncError::InvalidPath(message)) if message.contains("not a directory")),
        "{:?}",
        result.as_ref().err()
    );
}

#[test]
fn a_new_path_without_a_synced_ancestor_is_not_found_like_before() {
    let env = Env::new();
    env.download_done();
    let unsynced = env.dir.path().join("remote/elsewhere/deep/x.conf");
    let remote_path = unsynced.to_str().unwrap().to_owned();
    let local = local_path_for(&env.mirror_root(), HOST_KEY, &remote_path);
    fs::create_dir_all(local.parent().unwrap()).unwrap();
    fs::write(&local, "x").unwrap();
    let request = UploadRequest {
        remote_path,
        ..env.upload_request()
    };

    let result = block_on(prepare_upload(&env.shell, &request));

    assert!(
        matches!(&result, Err(WarpSyncError::NotFound(_))),
        "{:?}",
        result.as_ref().err()
    );
}

#[test]
fn a_path_that_exists_neither_on_the_server_nor_locally_is_not_found() {
    let env = Env::new();
    env.download_done();

    let result = env.prepare_new("nowhere/at/all.conf");

    assert!(
        matches!(&result, Err(WarpSyncError::NotFound(path)) if path.ends_with("/nowhere/at/all.conf")),
        "{:?}",
        result.as_ref().err()
    );
}

#[test]
fn a_directory_that_exists_on_the_server_but_is_not_synced_must_be_downloaded_first() {
    let env = Env::with_new_local_file("extra/new.conf", "new");
    fs::create_dir(env.remote("extra")).unwrap();

    let result = env.prepare_new("extra/new.conf");

    assert!(
        matches!(&result, Err(WarpSyncError::NotMirrored(path)) if path.ends_with("/conf/extra")),
        "{:?}",
        result.as_ref().err()
    );
}

#[test]
fn a_new_path_is_refused_for_another_mirror_folder_before_it_is_packed() {
    let env = Env::with_new_local_file("fresh.conf", "fresh");
    let request = UploadRequest {
        expected_host_key: Some("other-host".to_owned()),
        ..env.request_for_new("fresh.conf")
    };

    let result = block_on(prepare_upload(&env.shell, &request));

    assert!(
        matches!(&result, Err(WarpSyncError::Manifest(message)) if message.contains("another machine")),
        "{:?}",
        result.as_ref().err()
    );
}

#[test]
fn a_new_path_uses_the_mirror_of_the_machine_that_owns_it() {
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
    block_on(download(&machine_b, &env.download_request(false))).unwrap();
    let dir_b = machine_host_key(HOST_KEY, MACHINE_B);
    let local_b = local_path_for(&env.mirror_root(), &dir_b, &env.remote_path);
    fs::write(local_b.join("fresh.conf"), "fresh from B").unwrap();

    let prepared = block_on(prepare_upload(&machine_b, &env.request_for_new("fresh.conf"))).unwrap();
    block_on(execute_upload(&machine_b, &prepared)).unwrap();

    assert_eq!(prepared.host_key, dir_b);
    assert_eq!(
        fs::read_to_string(env.remote("fresh.conf")).unwrap(),
        "fresh from B"
    );
    assert!(
        load_manifest(&env.mirror_root(), HOST_KEY)
            .unwrap()
            .entries_under(&format!("{}/fresh.conf", env.remote_path))
            .is_empty(),
        "the other machine's manifest is not touched"
    );
}

#[test]
fn a_new_path_is_refused_when_the_session_no_longer_reaches_the_prepared_machine() {
    let env = Env::with_new_local_file("fresh.conf", "fresh");
    let prepared = env.prepare_new("fresh.conf").unwrap();
    let other_machine = AsMachine {
        shell: &env.shell,
        machine_id: MACHINE_B,
    };

    let result = block_on(execute_upload(&other_machine, &prepared));

    assert!(
        matches!(&result, Err(WarpSyncError::Manifest(message)) if message.contains("no longer reaches")),
        "{result:?}"
    );
    assert!(!env.remote("fresh.conf").exists());
}

#[test]
fn a_new_path_upload_probes_the_anchor_again_when_confirmed() {
    let env = Env::with_new_local_file("sub/fresh.conf", "fresh");
    let prepared = env.prepare_new("sub/fresh.conf").unwrap();
    fs::remove_dir_all(env.remote("sub")).unwrap();
    env.shell.calls.store(0, Ordering::SeqCst);

    let result = block_on(execute_upload(&env.shell, &prepared));

    assert!(
        matches!(&result, Err(WarpSyncError::NotFound(path)) if path.ends_with("/conf/sub")),
        "{result:?}"
    );
    assert_eq!(
        env.shell.calls.load(Ordering::SeqCst),
        1,
        "nothing but the probe reaches the host"
    );
    assert!(env.leftover_scratch_dirs().is_empty());
}

#[test]
fn a_new_path_upload_enters_the_git_baseline_and_leaves_other_edits_out_of_it() {
    let env = Env::with_new_local_file("one/two/only.conf", "only");
    fs::write(env.local("a.conf"), "edited a, not uploaded").unwrap();

    let outcome = env.upload_new("one/two/only.conf").unwrap();

    assert_eq!(outcome.baseline_warning, None);
    assert_eq!(
        baseline_contents(&env, "one/two/only.conf").as_deref(),
        Some("only")
    );
    assert_eq!(
        baseline_contents(&env, "a.conf").as_deref(),
        Some("remote a"),
        "what was not uploaded stays as the server has it"
    );
}

/// A host that names a different machine for every probe after the first two, as when the
/// terminal is moved to another host between the probes.
struct SwitchesMachine<'a> {
    shell: &'a LocalSh,
    probes: AtomicUsize,
}

#[async_trait]
impl RemoteShell for SwitchesMachine<'_> {
    async fn run(&self, command: &str) -> Result<Vec<u8>, WarpSyncError> {
        let output = self.shell.run(command).await?;
        let text = String::from_utf8_lossy(&output);
        if !text.contains("status=") {
            return Ok(output);
        }
        let probe = self.probes.fetch_add(1, Ordering::SeqCst) + 1;
        // 1: the path, 2: "/", 3 and later: the anchor.
        let machine_id = if probe <= 2 { MACHINE_A } else { MACHINE_B };
        let mut lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.starts_with("machine_id="))
            .collect();
        let machine_line = format!("machine_id={machine_id}");
        lines.push(&machine_line);
        Ok(lines.join("\n").into_bytes())
    }
}

#[test]
fn a_new_path_is_refused_when_the_host_changes_between_the_probes() {
    let env = Env::new();
    let machine_a = AsMachine {
        shell: &env.shell,
        machine_id: MACHINE_A,
    };
    block_on(download(&machine_a, &env.download_request(false))).unwrap();
    fs::write(env.local("fresh.conf"), "fresh").unwrap();
    let switching = SwitchesMachine {
        shell: &env.shell,
        probes: AtomicUsize::new(0),
    };

    let result = block_on(prepare_upload(&switching, &env.request_for_new("fresh.conf")));

    assert!(
        matches!(&result, Err(WarpSyncError::Manifest(message)) if message.contains("no longer reaches")),
        "{:?}",
        result.as_ref().err()
    );
}
