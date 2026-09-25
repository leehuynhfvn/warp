use std::ffi::OsString;
use std::path::PathBuf;

use super::*;

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn only_editors_with_a_diff_capable_launcher_qualify() {
    assert_eq!(
        EditorCli::for_editor(Editor::VSCode).map(|cli| cli.program),
        Some("code")
    );
    assert_eq!(
        EditorCli::for_editor(Editor::VSCodeInsiders).map(|cli| cli.program),
        Some("code-insiders")
    );
    assert_eq!(
        EditorCli::for_editor(Editor::Cursor).map(|cli| cli.program),
        Some("cursor")
    );
    assert_eq!(
        EditorCli::for_editor(Editor::Windsurf).map(|cli| cli.program),
        Some("windsurf")
    );
    assert_eq!(EditorCli::for_editor(Editor::Zed), None);
    assert_eq!(EditorCli::for_editor(Editor::IntelliJ), None);
}

#[test]
fn opening_the_mirror_opens_the_host_workspace() {
    let request = EditorRequest::OpenMirror {
        workspace: PathBuf::from("/m/host"),
        file: None,
    };
    assert_eq!(invocations(&request), vec![args(&["/m/host"])]);
}

#[test]
fn opening_a_file_keeps_the_host_workspace() {
    let request = EditorRequest::OpenMirror {
        workspace: PathBuf::from("/m/host"),
        file: Some(PathBuf::from("/m/host/etc/hosts")),
    };
    assert_eq!(
        invocations(&request),
        vec![args(&["/m/host", "/m/host/etc/hosts"])]
    );
}

#[test]
fn diffs_reuse_the_workspace_window() {
    let request = EditorRequest::OpenDiffs {
        workspace: PathBuf::from("/m/host"),
        diffs: vec![
            (PathBuf::from("/s/etc/a"), PathBuf::from("/m/host/etc/a")),
            (PathBuf::from("/s/etc/b"), PathBuf::from("/m/host/etc/b")),
        ],
        files: vec![PathBuf::from("/r/etc.diff")],
    };
    assert_eq!(
        invocations(&request),
        vec![
            args(&["/m/host"]),
            args(&["-r", "--diff", "/s/etc/a", "/m/host/etc/a"]),
            args(&["-r", "--diff", "/s/etc/b", "/m/host/etc/b"]),
            args(&["-r", "/r/etc.diff"]),
        ]
    );
}

#[test]
fn no_separate_invocation_without_extra_files() {
    let request = EditorRequest::OpenDiffs {
        workspace: PathBuf::from("/m/host"),
        diffs: vec![(PathBuf::from("/s/a"), PathBuf::from("/m/host/a"))],
        files: Vec::new(),
    };
    assert_eq!(invocations(&request).len(), 2);
}

#[test]
fn a_missing_launcher_explains_what_to_install() {
    let cli = EditorCli {
        program: "warp-sync-test-launcher-that-does-not-exist",
        name: "Test Editor",
    };
    let request = EditorRequest::OpenMirror {
        workspace: PathBuf::from("/"),
        file: None,
    };
    let Err(WarpSyncError::Editor(message)) = launch(cli, &request) else {
        panic!("expected an editor error");
    };
    assert!(message.contains("was not found"), "{message}");
    assert!(message.contains("Test Editor"), "{message}");
}

#[cfg(unix)]
#[test]
fn a_failing_launcher_is_reported() {
    let cli = EditorCli {
        program: "false",
        name: "Test Editor",
    };
    let request = EditorRequest::OpenMirror {
        workspace: PathBuf::from("/"),
        file: None,
    };
    assert!(matches!(
        launch(cli, &request),
        Err(WarpSyncError::Editor(_))
    ));
}
