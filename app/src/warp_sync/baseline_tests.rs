use std::fs;
use std::path::Path;

use command::blocking::Command;
use tempfile::TempDir;

use super::*;

fn write(host_dir: &Path, relative: &str, contents: &str) {
    let path = host_dir.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn git_output(host_dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(host_dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn baseline_files(host_dir: &Path) -> Vec<String> {
    git_output(host_dir, &["ls-tree", "-r", "--name-only", "HEAD"])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn baseline_contents(host_dir: &Path, relative: &str) -> String {
    git_output(host_dir, &["show", &format!("HEAD:{relative}")])
}

fn download(host_dir: &Path, remote_path: &str) -> BaselineOutcome {
    record_download(host_dir, remote_path, "Download").unwrap()
}

fn files(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|path| (*path).to_owned()).collect()
}

#[test]
fn the_first_download_creates_the_baseline() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/nginx/nginx.conf", "worker_processes 1;\n");
    write(&host, "etc/nginx/conf.d/site.conf", "server {}\n");

    assert_eq!(download(&host, "/etc/nginx"), BaselineOutcome::Recorded);

    assert_eq!(
        baseline_files(&host),
        ["etc/nginx/conf.d/site.conf", "etc/nginx/nginx.conf"]
    );
    assert_eq!(git_output(&host, &["status", "--porcelain"]), "");
}

#[test]
fn the_repository_does_not_record_where_the_mirror_is() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/hosts", "127.0.0.1\n");

    download(&host, "/etc/hosts");

    let config = fs::read_to_string(host.join(".git/config")).unwrap();
    assert!(!config.contains("worktree"), "{config}");
    assert!(config.contains("fileMode = false"), "{config}");
}

#[test]
fn settings_lost_after_an_interrupted_setup_are_restored() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/a", "a\n");
    download(&host, "/etc");
    git_output(&host, &["config", "core.fileMode", "true"]);
    fs::remove_file(host.join(".git/info/attributes")).unwrap();

    write(&host, "etc/a", "a2\n");
    download(&host, "/etc");

    assert_eq!(git_output(&host, &["config", "core.fileMode"]), "false\n");
    assert!(host.join(".git/info/attributes").exists());
}

#[test]
fn attributes_from_the_server_cannot_run_filters() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    let marker = dir.path().join("filter-ran");
    write(&host, "etc/a", "a\n");
    download(&host, "/etc");
    git_output(
        &host,
        &[
            "config",
            "filter.evil.clean",
            &format!("touch '{}'; cat", marker.display()),
        ],
    );

    write(&host, "etc/.gitattributes", "* filter=evil\n");
    write(&host, "etc/a", "a2\n");
    download(&host, "/etc");

    assert!(!marker.exists());
    assert_eq!(baseline_contents(&host, "etc/a"), "a2\n");
}

#[cfg(unix)]
#[test]
fn git_objects_are_private() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/shadow", "root:secret\n");
    download(&host, "/etc");

    let blob = git_output(&host, &["rev-parse", "HEAD:etc/shadow"]);
    let blob = blob.trim();
    let object = host.join(".git/objects").join(&blob[..2]).join(&blob[2..]);
    let mode = fs::metadata(object).unwrap().permissions().mode() & 0o077;
    assert_eq!(mode, 0);
}

#[test]
fn a_download_records_files_removed_from_the_server() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/a", "a\n");
    write(&host, "etc/b", "b\n");
    download(&host, "/etc");

    fs::remove_file(host.join("etc/b")).unwrap();
    write(&host, "etc/a", "a2\n");

    assert_eq!(download(&host, "/etc"), BaselineOutcome::Recorded);
    assert_eq!(baseline_files(&host), ["etc/a"]);
    assert_eq!(baseline_contents(&host, "etc/a"), "a2\n");
}

#[test]
fn a_download_leaves_edits_to_other_paths_alone() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/hosts", "old\n");
    write(&host, "srv/app.conf", "v1\n");
    download(&host, "/etc");
    download(&host, "/srv");

    write(&host, "etc/hosts", "edited locally\n");
    write(&host, "srv/app.conf", "v2\n");
    download(&host, "/srv");

    assert_eq!(baseline_contents(&host, "etc/hosts"), "old\n");
    assert_eq!(baseline_contents(&host, "srv/app.conf"), "v2\n");
    assert_eq!(
        git_output(&host, &["status", "--porcelain"]),
        " M etc/hosts\n"
    );
}

#[test]
fn what_the_user_staged_elsewhere_is_not_committed() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/hosts", "old\n");
    write(&host, "srv/app.conf", "v1\n");
    download(&host, "/etc");
    download(&host, "/srv");

    write(&host, "etc/hosts", "staged\n");
    git_output(&host, &["add", "etc/hosts"]);
    write(&host, "srv/app.conf", "v2\n");
    download(&host, "/srv");

    assert_eq!(baseline_contents(&host, "etc/hosts"), "old\n");
    assert_eq!(
        git_output(&host, &["diff", "--cached", "--name-only"]),
        "etc/hosts\n"
    );
}

#[test]
fn an_upload_keeps_files_deleted_locally_in_the_baseline() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/a", "a\n");
    write(&host, "etc/b", "b\n");
    download(&host, "/etc");

    write(&host, "etc/a", "edited\n");
    write(&host, "etc/new", "new\n");
    fs::remove_file(host.join("etc/b")).unwrap();
    let outcome = record_upload(&host, "/etc", &files(&["/etc/a", "/etc/new"]), "Upload").unwrap();

    assert_eq!(outcome, BaselineOutcome::Recorded);
    assert_eq!(baseline_files(&host), ["etc/a", "etc/b", "etc/new"]);
    assert_eq!(baseline_contents(&host, "etc/a"), "edited\n");
    assert_eq!(git_output(&host, &["status", "--porcelain"]), " D etc/b\n");
}

#[test]
fn an_upload_of_unchanged_files_records_nothing() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/a", "a\n");
    download(&host, "/etc");
    let head = git_output(&host, &["rev-parse", "HEAD"]);

    let outcome = record_upload(&host, "/etc", &files(&["/etc/a"]), "Upload").unwrap();

    assert_eq!(outcome, BaselineOutcome::Unchanged);
    assert_eq!(git_output(&host, &["rev-parse", "HEAD"]), head);
}

#[test]
fn ignore_files_from_the_server_do_not_hide_files() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/nginx/.gitignore", "*.conf\n");
    write(&host, "etc/nginx/nginx.conf", "events {}\n");

    download(&host, "/etc/nginx");

    assert_eq!(
        baseline_files(&host),
        ["etc/nginx/.gitignore", "etc/nginx/nginx.conf"]
    );
}

#[test]
fn names_are_never_read_as_patterns_or_options() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/*", "star\n");
    write(&host, "etc/other", "other\n");
    write(&host, "etc/--help", "dash\n");
    download(&host, "/etc");
    write(&host, "etc/*", "star2\n");
    write(&host, "etc/other", "other2\n");

    record_upload(&host, "/etc", &files(&["/etc/*"]), "Upload").unwrap();

    assert_eq!(baseline_contents(&host, "etc/*"), "star2\n");
    assert_eq!(baseline_contents(&host, "etc/other"), "other\n");
    assert_eq!(baseline_contents(&host, "etc/--help"), "dash\n");
}

#[test]
fn a_directory_without_files_is_unchanged() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    fs::create_dir_all(host.join("var/empty")).unwrap();

    assert_eq!(download(&host, "/var/empty"), BaselineOutcome::Unchanged);
    assert_eq!(baseline_files(&host), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn hooks_in_the_repository_never_run() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/a", "a\n");
    download(&host, "/etc");
    let marker = dir.path().join("hook-ran");
    let hook = host.join(".git/hooks/pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

    write(&host, "etc/a", "a2\n");
    assert_eq!(download(&host, "/etc"), BaselineOutcome::Recorded);

    assert!(!marker.exists());
}

#[test]
fn a_missing_git_leaves_the_mirror_alone() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/a", "a\n");

    let outcome = record_download_with(
        "warp-sync-test-git-that-does-not-exist",
        &host,
        "/etc",
        "Download",
    )
    .unwrap();

    assert_eq!(outcome, BaselineOutcome::GitUnavailable);
    assert!(!host.join(".git").exists());
}

#[test]
fn a_git_entry_that_is_not_a_directory_is_an_error() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, ".git", "gitdir: /elsewhere\n");
    write(&host, "etc/a", "a\n");

    assert!(matches!(
        record_download(&host, "/etc", "Download"),
        Err(WarpSyncError::Baseline(_))
    ));
}

#[test]
fn status_output_is_split_into_paths() {
    let output = b" M etc/a\0?? etc/new file\0!! etc/ignored\0 D etc/b\0";

    assert_eq!(
        parse_status(output),
        ["etc/a", "etc/new file", "etc/ignored", "etc/b"]
    );
}

#[test]
fn commit_messages_cannot_add_lines() {
    assert_eq!(
        commit_message("Upload", "/etc/a\rb", "root"),
        "Upload /etc/a\\rb as root"
    );
}

#[test]
fn the_synced_content_is_what_was_last_recorded_not_the_edited_copy() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/nginx/nginx.conf", "worker_processes 1;\n");
    download(&host, "/etc/nginx/nginx.conf");
    write(&host, "etc/nginx/nginx.conf", "worker_processes 4;\n");

    assert_eq!(
        synced_content(&host, "/etc/nginx/nginx.conf"),
        Some(b"worker_processes 1;\n".to_vec())
    );
}

#[test]
fn there_is_no_synced_content_without_a_baseline_or_for_an_unknown_file() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("prod-1");
    write(&host, "etc/hosts", "127.0.0.1 localhost\n");

    assert_eq!(synced_content(&host, "/etc/hosts"), None);
    assert!(!host.join(GIT_DIR_NAME).exists());

    download(&host, "/etc/hosts");
    assert_eq!(synced_content(&host, "/etc/fstab"), None);
}
