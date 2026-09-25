use std::path::PathBuf;

use uuid::Uuid;

use super::*;
use crate::warp_sync::remote_check::RemoteConflicts;
use crate::warp_sync::requester::SkippedEntry;

fn upload_summary(remote_check: RemoteCheck) -> UploadSummary {
    UploadSummary {
        remote_user: "root".to_owned(),
        hostname: "prod-1".to_owned(),
        remote_path: "/etc/nginx".to_owned(),
        files: 2,
        dirs: 1,
        content_bytes: 3000,
        new_files: vec!["/etc/nginx/new.conf".to_owned()],
        missing_locally: vec!["/etc/nginx/old.conf".to_owned()],
        remote_check,
        ownership_may_be_incomplete: true,
        server_id_tail: Some("cdef".to_owned()),
    }
}

#[test]
fn a_download_keeps_its_counts_and_skipped_entries() {
    let result = sync_result(SyncReply::Downloaded {
        local_path: PathBuf::from("/m/prod-1/etc"),
        files: 3,
        dirs: 2,
        bytes: 4096,
        remote_user: "root".to_owned(),
        skipped: vec![SkippedEntry {
            path: "/etc/link".to_owned(),
            reason: "symbolic link".to_owned(),
        }],
        baseline_warning: Some("no git".to_owned()),
    });

    assert_eq!(
        result,
        SyncResult::Downloaded {
            local_path: "/m/prod-1/etc".to_owned(),
            files: 3,
            dirs: 2,
            bytes: 4096,
            remote_user: "root".to_owned(),
            skipped: vec![SyncSkippedEntry {
                path: "/etc/link".to_owned(),
                reason: "symbolic link".to_owned(),
            }],
            baseline_warning: Some("no git".to_owned()),
        }
    );
}

#[test]
fn an_overwrite_confirmation_carries_the_pending_id_and_files() {
    let pending_id = Uuid::new_v4();

    let result = sync_result(SyncReply::NeedsConfirmation {
        pending_id,
        kind: ConfirmationKind::OverwriteLocalChanges {
            files: vec!["/etc/a".to_owned()],
        },
    });

    assert_eq!(
        result,
        SyncResult::NeedsConfirmation {
            pending_id,
            confirmation: SyncConfirmation::OverwriteLocalChanges {
                files: vec!["/etc/a".to_owned()]
            },
        }
    );
}

#[test]
fn an_upload_confirmation_exposes_the_conflicts_the_host_check_found() {
    let conflicts = RemoteConflicts {
        changed: vec!["/etc/nginx/nginx.conf".to_owned()],
        missing: vec!["/etc/nginx/gone.conf".to_owned()],
        already_exist: vec!["/etc/nginx/new.conf".to_owned()],
    };

    let result = sync_result(SyncReply::NeedsConfirmation {
        pending_id: Uuid::new_v4(),
        kind: ConfirmationKind::Upload(Box::new(upload_summary(RemoteCheck::Checked(conflicts)))),
    });

    let SyncResult::NeedsConfirmation {
        confirmation: SyncConfirmation::Upload { summary },
        ..
    } = result
    else {
        panic!("expected an upload confirmation");
    };
    assert_eq!(summary.remote_user, "root");
    assert_eq!((summary.files, summary.dirs, summary.bytes), (2, 1, 3000));
    assert_eq!(summary.missing_locally, vec!["/etc/nginx/old.conf".to_owned()]);
    assert!(summary.ownership_may_be_incomplete);
    assert_eq!(summary.server_id_tail.as_deref(), Some("cdef"));
    let conflicts = summary.remote_conflicts.expect("the host was checked");
    assert_eq!(conflicts.changed, vec!["/etc/nginx/nginx.conf".to_owned()]);
    assert_eq!(conflicts.missing, vec!["/etc/nginx/gone.conf".to_owned()]);
    assert_eq!(conflicts.already_exist, vec!["/etc/nginx/new.conf".to_owned()]);
}

#[test]
fn an_unchecked_host_has_no_conflict_report() {
    let result = sync_result(SyncReply::NeedsConfirmation {
        pending_id: Uuid::new_v4(),
        kind: ConfirmationKind::Upload(Box::new(upload_summary(RemoteCheck::Unavailable))),
    });

    let SyncResult::NeedsConfirmation {
        confirmation: SyncConfirmation::Upload { summary },
        ..
    } = result
    else {
        panic!("expected an upload confirmation");
    };
    assert!(summary.remote_conflicts.is_none());
}

#[test]
fn a_comparison_marks_which_files_can_be_shown_side_by_side() {
    let result = sync_result(SyncReply::Compared {
        differences: vec![
            FileDifference {
                remote_path: "/etc/a".to_owned(),
                change: FileChange::ChangedOnServer,
            },
            FileDifference {
                remote_path: "/etc/b".to_owned(),
                change: FileChange::NewOnServer,
            },
            FileDifference {
                remote_path: "/etc/c".to_owned(),
                change: FileChange::ChangedUnknown,
            },
        ],
        identical_files: 7,
        diff_path: PathBuf::from("/m/.warp-sync/diffs/h/etc.diff"),
        host_dir: PathBuf::from("/m/h"),
        server_copy_dir: PathBuf::from("/m/.warp-sync/compare/h"),
        remote_user: "root".to_owned(),
    });

    let SyncResult::Compared {
        differences,
        identical_files,
        diff_path,
        host_dir,
        server_copy_dir,
        remote_user,
    } = result
    else {
        panic!("expected a comparison");
    };
    assert_eq!(identical_files, 7);
    assert_eq!(diff_path, "/m/.warp-sync/diffs/h/etc.diff");
    assert_eq!(host_dir, "/m/h");
    assert_eq!(server_copy_dir, "/m/.warp-sync/compare/h");
    assert_eq!(remote_user, "root");
    let summary: Vec<_> = differences
        .iter()
        .map(|difference| (difference.change, difference.on_both_sides))
        .collect();
    assert_eq!(
        summary,
        [
            (SyncChange::ChangedOnServer, true),
            (SyncChange::NewOnServer, false),
            (SyncChange::Differs, true),
        ]
    );
}

#[test]
fn an_upload_and_an_unchanged_comparison_keep_their_numbers() {
    assert_eq!(
        sync_result(SyncReply::Uploaded {
            files: 1,
            dirs: 0,
            bytes: 9,
            remote_user: "root".to_owned(),
            backup_path: Some("/root/.warp-sync/backups/b.tgz".to_owned()),
            baseline_warning: None,
        }),
        SyncResult::Uploaded {
            files: 1,
            dirs: 0,
            bytes: 9,
            remote_user: "root".to_owned(),
            backup_path: Some("/root/.warp-sync/backups/b.tgz".to_owned()),
            baseline_warning: None,
        }
    );
    assert_eq!(
        sync_result(SyncReply::Unchanged { identical_files: 4 }),
        SyncResult::Unchanged { identical_files: 4 }
    );
}

#[test]
fn failures_map_to_codes_a_client_can_act_on() {
    let cases = [
        (WarpSyncError::InvalidPath("x".to_owned()), ErrorCode::InvalidParams),
        (WarpSyncError::NoSession("prod-1".to_owned()), ErrorCode::MissingTarget),
        (
            WarpSyncError::AmbiguousSession("two".to_owned()),
            ErrorCode::AmbiguousTarget,
        ),
        (WarpSyncError::PendingNotFound, ErrorCode::StaleTarget),
        (WarpSyncError::AlreadyInProgress, ErrorCode::TargetStateConflict),
        (WarpSyncError::Timeout, ErrorCode::SyncFailed),
        (WarpSyncError::TooManyPending, ErrorCode::SyncFailed),
        (
            WarpSyncError::PermissionDenied {
                user: "alice".to_owned(),
            },
            ErrorCode::SyncFailed,
        ),
        (
            WarpSyncError::Manifest("the mirror belongs to another machine".to_owned()),
            ErrorCode::SyncFailed,
        ),
    ];

    for (error, code) in cases {
        let message = error.to_string();
        let control = control_error(error);
        assert_eq!(control.code, code, "{message}");
        assert_eq!(control.message, message);
    }
}

#[test]
fn error_messages_that_quote_the_server_are_safe_to_print_in_a_terminal() {
    let error = WarpSyncError::RemoteCommandFailed {
        exit_code: Some(1),
        message: "boom\u{1b}]52;c;AAAA\u{7}\nfake: line".to_owned(),
    };

    let control = control_error(error);

    assert!(!control.message.contains('\u{1b}'));
    assert!(!control.message.contains('\u{7}'));
    assert!(!control.message.contains('\n'));
    assert!(control.message.contains("boom"));
}
