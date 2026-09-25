use std::collections::BTreeMap;
use std::path::PathBuf;

use super::super::archive::SkipReason;
use super::super::diff::FileChange;
use super::*;

fn download_outcome() -> DownloadOutcome {
    DownloadOutcome {
        host_dir: PathBuf::from("/m/h"),
        local_path: PathBuf::from("/m/h/etc/nginx"),
        is_file: false,
        files: 3,
        dirs: 2,
        total_bytes: 4096,
        skipped: vec![("/etc/nginx/ev\nil".to_owned(), SkipReason::Symlink)],
        remote_user: "root".to_owned(),
        baseline_warning: Some("git\nfailed".to_owned()),
    }
}

fn compare_outcome(diff_path: Option<PathBuf>) -> CompareOutcome {
    CompareOutcome {
        differences: vec![FileDifference {
            remote_path: "/etc/a\tb".to_owned(),
            change: FileChange::ChangedLocally,
        }],
        identical_files: 5,
        diff_path,
        remote_user: "root".to_owned(),
        host_dir: PathBuf::from("/m/h"),
        server_copy_dir: PathBuf::from("/m/.warp-sync/compare/h"),
    }
}

#[test]
fn a_download_reply_carries_the_counts_and_escapes_server_strings() {
    let reply = Finished::Downloaded {
        remote_path: "/etc/nginx".to_owned(),
        outcome: download_outcome(),
    }
    .into_reply()
    .expect("a download reply");

    let SyncReply::Downloaded {
        local_path,
        files,
        dirs,
        bytes,
        remote_user,
        skipped,
        baseline_warning,
    } = reply
    else {
        panic!("expected a download reply");
    };
    assert_eq!(local_path, PathBuf::from("/m/h/etc/nginx"));
    assert_eq!((files, dirs, bytes), (3, 2, 4096));
    assert_eq!(remote_user, "root");
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].path, "/etc/nginx/ev\\nil");
    assert_eq!(skipped[0].reason, "symbolic link");
    assert_eq!(baseline_warning.as_deref(), Some("git\\nfailed"));
}

#[test]
fn an_upload_reply_escapes_the_backup_path() {
    let reply = Finished::Uploaded {
        remote_path: "/etc/nginx".to_owned(),
        outcome: UploadOutcome {
            files: 1,
            dirs: 1,
            content_bytes: 10,
            backup_path: Some("/root/.warp-sync/b\n.tgz".to_owned()),
            remote_user: "root".to_owned(),
            baseline_warning: None,
        },
    }
    .into_reply()
    .expect("an upload reply");

    let SyncReply::Uploaded {
        backup_path, bytes, ..
    } = reply
    else {
        panic!("expected an upload reply");
    };
    assert_eq!(backup_path.as_deref(), Some("/root/.warp-sync/b\\n.tgz"));
    assert_eq!(bytes, 10);
}

#[test]
fn a_comparison_with_differences_lists_them_with_escaped_paths() {
    let reply = Finished::Compared {
        hostname: "prod-1".to_owned(),
        remote_path: "/etc".to_owned(),
        outcome: compare_outcome(Some(PathBuf::from("/m/etc.diff"))),
    }
    .into_reply()
    .expect("a comparison reply");

    let SyncReply::Compared {
        differences,
        identical_files,
        diff_path,
        ..
    } = reply
    else {
        panic!("expected a comparison reply");
    };
    assert_eq!(diff_path, PathBuf::from("/m/etc.diff"));
    assert_eq!(identical_files, 5);
    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].remote_path, "/etc/a\\tb");
    assert_eq!(differences[0].change, FileChange::ChangedLocally);
}

#[test]
fn a_comparison_without_differences_is_unchanged() {
    let reply = Finished::Compared {
        hostname: "prod-1".to_owned(),
        remote_path: "/etc".to_owned(),
        outcome: compare_outcome(None),
    }
    .into_reply()
    .expect("a comparison reply");

    assert!(matches!(reply, SyncReply::Unchanged { identical_files: 5 }));
}

#[test]
fn a_failure_becomes_an_error_reply() {
    let reply = Finished::Failed(WarpSyncError::Timeout).into_reply();

    assert_eq!(reply.unwrap_err(), WarpSyncError::Timeout);
}

#[test]
fn an_upload_confirmation_escapes_every_server_string() {
    let summary = UploadSummary {
        remote_user: "ro\not".to_owned(),
        hostname: "pr\nod".to_owned(),
        remote_path: "/etc/a\nb".to_owned(),
        files: 1,
        dirs: 1,
        content_bytes: 1,
        new_files: vec!["/etc/n\new".to_owned()],
        new_modes: BTreeMap::from([("/etc/n\new".to_owned(), 0o600)]),
        creates_under: Some("/et\nc".to_owned()),
        missing_locally: vec!["/etc/m\tissing".to_owned()],
        remote_check: RemoteCheck::Checked(RemoteConflicts {
            changed: vec!["/etc/c\rhanged".to_owned()],
            missing: vec!["/etc/g\none".to_owned()],
            already_exist: vec!["/etc/e\x1bxists".to_owned()],
        }),
        ownership_may_be_incomplete: false,
        server_id_tail: Some("ab\ncd".to_owned()),
    };

    let ConfirmationKind::Upload(summary) = ConfirmationKind::upload(summary) else {
        panic!("expected an upload confirmation");
    };

    assert_eq!(summary.remote_user, "ro\\not");
    assert_eq!(summary.hostname, "pr\\nod");
    assert_eq!(summary.remote_path, "/etc/a\\nb");
    assert_eq!(summary.new_files, vec!["/etc/n\\new".to_owned()]);
    assert_eq!(
        summary.new_modes,
        BTreeMap::from([("/etc/n\\new".to_owned(), 0o600)])
    );
    assert_eq!(summary.creates_under.as_deref(), Some("/et\\nc"));
    assert_eq!(summary.missing_locally, vec!["/etc/m\\tissing".to_owned()]);
    assert_eq!(summary.server_id_tail.as_deref(), Some("ab\\ncd"));
    let RemoteCheck::Checked(conflicts) = summary.remote_check else {
        panic!("expected a checked host");
    };
    assert_eq!(conflicts.changed, vec!["/etc/c\\rhanged".to_owned()]);
    assert_eq!(conflicts.missing, vec!["/etc/g\\none".to_owned()]);
    assert_eq!(
        conflicts.already_exist,
        vec!["/etc/e\\u{1b}xists".to_owned()]
    );
    assert_eq!((summary.files, summary.content_bytes), (1, 1));
}

#[test]
fn an_unavailable_remote_check_stays_unavailable() {
    let summary = UploadSummary {
        remote_user: "root".to_owned(),
        hostname: "prod-1".to_owned(),
        remote_path: "/etc".to_owned(),
        files: 0,
        dirs: 0,
        content_bytes: 0,
        new_files: Vec::new(),
        new_modes: BTreeMap::new(),
        creates_under: None,
        missing_locally: Vec::new(),
        remote_check: RemoteCheck::Unavailable,
        ownership_may_be_incomplete: true,
        server_id_tail: None,
    };

    let ConfirmationKind::Upload(summary) = ConfirmationKind::upload(summary) else {
        panic!("expected an upload confirmation");
    };

    assert_eq!(summary.remote_check, RemoteCheck::Unavailable);
    assert!(summary.ownership_may_be_incomplete);
}

#[test]
fn local_changes_to_overwrite_are_escaped() {
    let ConfirmationKind::OverwriteLocalChanges { files } =
        ConfirmationKind::overwrite_local_changes(vec!["/etc/a\nb".to_owned()])
    else {
        panic!("expected an overwrite confirmation");
    };

    assert_eq!(files, vec!["/etc/a\\nb".to_owned()]);
}

#[test]
fn a_reply_to_a_client_that_left_is_dropped_quietly() {
    let (reply, receiver) = ExternalReply::channel();
    drop(receiver);

    reply.send(Err(WarpSyncError::Timeout));
}
