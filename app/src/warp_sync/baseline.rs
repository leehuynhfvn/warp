//! Keeps a Git repository at the root of each host's mirror whose `HEAD` is what the server had at
//! the last sync, so that an editor's source control view shows exactly the edits that have not
//! been uploaded yet. Everything here blocks on `git`, so callers run it on a background executor.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use command::Stdio;
use command::blocking::Command;

use super::WarpSyncError;
use super::paths::{GIT_DIR_NAME, printable};

const GIT_PROGRAM: &str = "git";
const INITIAL_BRANCH: &str = "main";
const INITIAL_COMMIT_MESSAGE: &str = "Warp Sync baseline";

/// Kept out of the baseline: an editor may create it at the root of the workspace.
const EXCLUDES: &str = "/.vscode/\n";

/// Takes precedence over every `.gitattributes` in the mirror, which the server decides: no
/// attribute may hand file contents to a filter or diff driver from the user's own configuration,
/// or change them on the way into the baseline.
const ATTRIBUTES: &str = "* !filter !diff !merge !text !eol !ident !working-tree-encoding\n";

/// Git objects hold copies of files that may be readable by root only on the server.
const SHARED_REPOSITORY: &str = "--shared=0600";

/// Stored in the repository so that the editor's own `git` agrees with the baseline: local modes
/// are not what gets uploaded (the manifest keeps the server's modes).
const REPOSITORY_CONFIG: [(&str, &str); 3] = [
    ("core.fileMode", "false"),
    ("core.autocrlf", "false"),
    ("core.sharedRepository", "0600"),
];

/// Applied to every command. The user's configuration must not sign commits or run hooks, and
/// nothing from the mirror, which the server decides, may make `git` run a program.
const COMMAND_CONFIG: [&str; 6] = [
    "core.hooksPath=/dev/null",
    "core.fsmonitor=false",
    "commit.gpgsign=false",
    "core.autocrlf=false",
    "user.name=Warp Sync",
    "user.email=warp-sync@localhost",
];

/// Two syncs of different paths on the same host would otherwise race for the index lock.
static BASELINE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselineOutcome {
    Recorded,
    /// The baseline already matched.
    Unchanged,
    /// `git` is not installed, so there is no baseline.
    GitUnavailable,
}

/// Records that the mirror of `remote_path` under `host_dir` now matches the server, including
/// files that the server no longer has.
pub fn record_download(
    host_dir: &Path,
    remote_path: &str,
    message: &str,
) -> Result<BaselineOutcome, WarpSyncError> {
    record_download_with(GIT_PROGRAM, host_dir, remote_path, message)
}

/// Records that the server now has the mirror's copy of `files` (absolute remote paths) under
/// `remote_path`. Files deleted locally keep their baseline, since uploading does not delete them
/// on the server.
pub fn record_upload(
    host_dir: &Path,
    remote_path: &str,
    files: &BTreeSet<String>,
    message: &str,
) -> Result<BaselineOutcome, WarpSyncError> {
    record_upload_with(GIT_PROGRAM, host_dir, remote_path, files, message)
}

fn record_download_with(
    program: &str,
    host_dir: &Path,
    remote_path: &str,
    message: &str,
) -> Result<BaselineOutcome, WarpSyncError> {
    let _lock = BASELINE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let git = Git::new(program, host_dir);
    if !git.ensure_repository()? {
        return Ok(BaselineOutcome::GitUnavailable);
    }
    let pathspec = relative_path(remote_path);
    // Staging a path that matches nothing is an error, so an unchanged subtree (or one that holds
    // no files at all) must stop here.
    if git.changed_paths(&pathspec)?.is_empty() {
        return Ok(BaselineOutcome::Unchanged);
    }
    let pathspecs = [pathspec];
    git.run(&["add", "--all", "--force"], Some(&pathspecs))?;
    git.commit(message, &pathspecs)?;
    Ok(BaselineOutcome::Recorded)
}

fn record_upload_with(
    program: &str,
    host_dir: &Path,
    remote_path: &str,
    files: &BTreeSet<String>,
    message: &str,
) -> Result<BaselineOutcome, WarpSyncError> {
    let _lock = BASELINE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let git = Git::new(program, host_dir);
    if !git.ensure_repository()? {
        return Ok(BaselineOutcome::GitUnavailable);
    }
    let uploaded: BTreeSet<String> = files.iter().map(|file| relative_path(file)).collect();
    let pathspecs: Vec<String> = git
        .changed_paths(&relative_path(remote_path))?
        .into_iter()
        .filter(|path| uploaded.contains(path))
        .collect();
    if pathspecs.is_empty() {
        return Ok(BaselineOutcome::Unchanged);
    }
    git.run(&["add", "--force"], Some(&pathspecs))?;
    git.commit(message, &pathspecs)?;
    Ok(BaselineOutcome::Recorded)
}

/// The content `remote_path` had when it was last synced, from the baseline under `host_dir`.
/// `None` when there is no baseline, the file is not in it, or `git` cannot be run; nothing is
/// created.
pub fn synced_content(host_dir: &Path, remote_path: &str) -> Option<Vec<u8>> {
    synced_content_with(GIT_PROGRAM, host_dir, remote_path)
}

fn synced_content_with(program: &str, host_dir: &Path, remote_path: &str) -> Option<Vec<u8>> {
    let git = Git::new(program, host_dir);
    if !git.git_dir.is_dir() {
        return None;
    }
    let object = format!("HEAD:{}", relative_path(remote_path));
    let command = git.command(&["cat-file", "blob", &object]);
    match git.execute_raw(command, None) {
        Ok(output) if output.status.success() => Some(output.stdout),
        Ok(_) | Err(GitFailure::NotInstalled) | Err(GitFailure::Failed(_)) => None,
    }
}

/// A commit message that names the synced path without letting it add lines.
pub fn commit_message(action: &str, remote_path: &str, remote_user: &str) -> String {
    format!(
        "{action} {} as {}",
        printable(remote_path),
        printable(remote_user)
    )
}

/// The mirror-relative path of a normalized absolute remote path.
fn relative_path(remote_path: &str) -> String {
    remote_path.trim_start_matches('/').to_owned()
}

struct Git<'a> {
    program: &'a str,
    host_dir: &'a Path,
    git_dir: PathBuf,
}

impl<'a> Git<'a> {
    fn new(program: &'a str, host_dir: &'a Path) -> Self {
        Self {
            program,
            host_dir,
            git_dir: host_dir.join(GIT_DIR_NAME),
        }
    }

    /// Creates the repository and its first commit if needed. Returns `false` when `git` is not
    /// installed.
    fn ensure_repository(&self) -> Result<bool, WarpSyncError> {
        match fs::symlink_metadata(&self.git_dir) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(WarpSyncError::Baseline(format!(
                    "{} is not a directory",
                    self.git_dir.display()
                )));
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                // Without `--git-dir`, so that the repository does not record where the mirror is.
                let mut init = self.base_command();
                init.args(["-c", &format!("init.defaultBranch={INITIAL_BRANCH}")])
                    .args(["init", "--quiet", SHARED_REPOSITORY, "--"])
                    .arg(self.host_dir);
                match self.execute(init, None) {
                    Ok(_) => {}
                    Err(GitFailure::NotInstalled) => return Ok(false),
                    Err(GitFailure::Failed(err)) => return Err(err),
                }
            }
            Err(err) => {
                return Err(WarpSyncError::Baseline(format!(
                    "could not read {}: {err}",
                    self.git_dir.display()
                )));
            }
        }
        self.configure()?;
        if !self.has_head()? {
            self.run(
                &[
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "--message",
                    INITIAL_COMMIT_MESSAGE,
                ],
                None,
            )?;
        }
        Ok(true)
    }

    /// Applied on every sync, so that a repository whose setup was interrupted, or whose settings
    /// were edited, is put right.
    fn configure(&self) -> Result<(), WarpSyncError> {
        for (key, value) in REPOSITORY_CONFIG {
            self.run(&["config", key, value], None)?;
        }
        let info_dir = self.git_dir.join("info");
        fs::create_dir_all(&info_dir)
            .and_then(|()| fs::write(info_dir.join("exclude"), EXCLUDES))
            .and_then(|()| fs::write(info_dir.join("attributes"), ATTRIBUTES))
            .map_err(|err| {
                WarpSyncError::Baseline(format!("could not write the Git info files: {err}"))
            })
    }

    fn has_head(&self) -> Result<bool, WarpSyncError> {
        let command = self.command(&["rev-parse", "--quiet", "--verify", "HEAD^{commit}"]);
        match self.execute_raw(command, None) {
            Ok(output) => Ok(output.status.success()),
            Err(GitFailure::NotInstalled) => Err(not_installed()),
            Err(GitFailure::Failed(err)) => Err(err),
        }
    }

    /// Paths under `pathspec` whose working copy differs from `HEAD` or from the index, including
    /// untracked and ignored files. Paths are relative to the host directory.
    fn changed_paths(&self, pathspec: &str) -> Result<Vec<String>, WarpSyncError> {
        let output = self.run(
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignored=matching",
                "--no-renames",
                "--",
                pathspec,
            ],
            None,
        )?;
        Ok(parse_status(&output))
    }

    /// Commits the working copy of exactly `pathspecs`, leaving alone whatever else is staged.
    fn commit(&self, message: &str, pathspecs: &[String]) -> Result<(), WarpSyncError> {
        self.run(
            &["commit", "--quiet", "--only", "--message", message],
            Some(pathspecs),
        )
        .map(|_| ())
    }

    /// Runs `git` with `args`; when `pathspecs` is given, they are passed NUL-separated on stdin
    /// so that no name can be read as an option.
    fn run(&self, args: &[&str], pathspecs: Option<&[String]>) -> Result<Vec<u8>, WarpSyncError> {
        let mut command = self.command(args);
        let stdin = pathspecs.map(|pathspecs| {
            command.args(["--pathspec-from-file=-", "--pathspec-file-nul"]);
            let mut list = Vec::new();
            for pathspec in pathspecs {
                list.extend_from_slice(pathspec.as_bytes());
                list.push(0);
            }
            list
        });
        match self.execute(command, stdin.as_deref()) {
            Ok(stdout) => Ok(stdout),
            Err(GitFailure::NotInstalled) => Err(not_installed()),
            Err(GitFailure::Failed(err)) => Err(err),
        }
    }

    /// A command for this repository, whatever the environment says.
    fn command(&self, args: &[&str]) -> Command {
        let mut git_dir = OsString::from("--git-dir=");
        git_dir.push(&self.git_dir);
        let mut work_tree = OsString::from("--work-tree=");
        work_tree.push(self.host_dir);
        let mut command = self.base_command();
        command.arg(git_dir).arg(work_tree).args(args);
        command
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new(self.program);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.arg("--literal-pathspecs");
        for config in COMMAND_CONFIG {
            command.args(["-c", config]);
        }
        command
    }

    fn execute(&self, command: Command, stdin: Option<&[u8]>) -> Result<Vec<u8>, GitFailure> {
        let output = self.execute_raw(command, stdin)?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(GitFailure::Failed(WarpSyncError::Baseline(format!(
            "git exited with {}: {}",
            output.status,
            printable(stderr.trim())
        ))))
    }

    fn execute_raw(
        &self,
        mut command: Command,
        stdin: Option<&[u8]>,
    ) -> Result<std::process::Output, GitFailure> {
        command
            .current_dir(self.host_dir)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|err| match err.kind() {
            ErrorKind::NotFound => GitFailure::NotInstalled,
            _ => GitFailure::Failed(WarpSyncError::Baseline(format!("could not run git: {err}"))),
        })?;
        if let Some(input) = stdin
            && let Some(mut pipe) = child.stdin.take()
        {
            pipe.write_all(input).map_err(|err| {
                GitFailure::Failed(WarpSyncError::Baseline(format!(
                    "could not pass paths to git: {err}"
                )))
            })?;
        }
        child.wait_with_output().map_err(|err| {
            GitFailure::Failed(WarpSyncError::Baseline(format!("could not run git: {err}")))
        })
    }
}

enum GitFailure {
    NotInstalled,
    Failed(WarpSyncError),
}

fn not_installed() -> WarpSyncError {
    WarpSyncError::Baseline("git is no longer available".to_owned())
}

/// The paths in `git status --porcelain=v1 -z --no-renames` output.
fn parse_status(output: &[u8]) -> Vec<String> {
    output
        .split(|byte| *byte == 0)
        .filter_map(|record| record.get(3..).filter(|path| !path.is_empty()))
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

#[cfg(test)]
#[path = "baseline_tests.rs"]
mod tests;
