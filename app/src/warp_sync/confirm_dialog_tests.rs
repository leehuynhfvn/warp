use std::collections::BTreeMap;

use super::super::diff::{FileChange, FileDifference};
use super::super::risk::UploadRisks;
use super::*;
use crate::util::file::external_editor::Editor;

fn paths(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("/etc/f{i}")).collect()
}

fn summary() -> UploadSummary {
    UploadSummary {
        remote_user: "root".to_owned(),
        hostname: "prod-1".to_owned(),
        remote_path: "/etc/nginx".to_owned(),
        files: 3,
        dirs: 1,
        content_bytes: 2048,
        new_files: vec![],
        new_modes: BTreeMap::new(),
        creates_under: None,
        risks: UploadRisks::default(),
        missing_locally: vec![],
        remote_check: RemoteCheck::Checked(RemoteConflicts::default()),
        ownership_may_be_incomplete: false,
        server_id_tail: Some("ab12".to_owned()),
    }
}

#[test]
fn bullet_list_shows_every_path_up_to_the_limit() {
    let list = bullet_list(&paths(MAX_LISTED_PATHS));

    assert_eq!(list.lines().count(), MAX_LISTED_PATHS);
    assert!(!list.contains("more"));
}

#[test]
fn bullet_list_summarizes_the_rest() {
    let list = bullet_list(&paths(MAX_LISTED_PATHS + 3));

    assert_eq!(list.lines().count(), MAX_LISTED_PATHS + 1);
    assert!(list.ends_with("… and 3 more"));
}

#[test]
fn upload_body_names_the_target_and_the_backup_location() {
    let body = upload_body(&summary());

    assert!(
        body.starts_with("prod-1 (machine id …ab12):/etc/nginx\n\n3 files and 1 folder, 2.0 KiB")
    );
    assert!(body.contains("~/.warp-sync/backups"));
    assert!(!body.contains("New on the server"));
    assert!(!body.contains("NOT be deleted"));
    assert!(!body.contains("not GNU tar"));
}

#[test]
fn upload_body_lists_new_and_missing_files_and_warns_about_tar() {
    let summary = UploadSummary {
        new_files: vec!["/etc/nginx/new.conf".to_owned()],
        missing_locally: vec!["/etc/nginx/old.conf".to_owned()],
        ownership_may_be_incomplete: true,
        ..summary()
    };

    let body = upload_body(&summary);

    assert!(body.contains("New on the server:\n• /etc/nginx/new.conf"));
    assert!(body.contains("NOT be deleted on the server):\n• /etc/nginx/old.conf"));
    assert!(body.contains("not GNU tar"));
}

#[test]
fn upload_body_shows_the_mode_each_new_entry_will_get() {
    let summary = UploadSummary {
        new_files: vec![
            "/etc/nginx/secret.conf".to_owned(),
            "/etc/nginx/plain.conf".to_owned(),
        ],
        new_modes: BTreeMap::from([("/etc/nginx/secret.conf".to_owned(), 0o600)]),
        ..summary()
    };

    let body = upload_body(&summary);

    assert!(body.contains("• /etc/nginx/secret.conf (mode 0600)\n• /etc/nginx/plain.conf\n"));
}

#[test]
fn upload_body_says_what_will_be_created_and_where_when_the_path_is_new() {
    let summary = UploadSummary {
        remote_path: "/root/test-dir-2".to_owned(),
        creates_under: Some("/root".to_owned()),
        new_files: vec![
            "/root/test-dir-2".to_owned(),
            "/root/test-dir-2/a.conf".to_owned(),
        ],
        new_modes: BTreeMap::from([
            ("/root/test-dir-2".to_owned(), 0o755),
            ("/root/test-dir-2/a.conf".to_owned(), 0o644),
        ]),
        remote_check: RemoteCheck::Checked(RemoteConflicts::default()),
        ..summary()
    };

    let body = upload_body(&summary);

    assert!(body.contains(
        "Creates on the server, inside /root:\n• /root/test-dir-2 (mode 0755)\n• /root/test-dir-2/a.conf (mode 0644)"
    ));
    assert!(body.contains("Nothing on the server is replaced"));
    assert!(!body.contains("~/.warp-sync/backups"));
    assert!(!body.contains("unchanged since the last sync"));
    assert!(!body.contains("New on the server"));
}

#[test]
fn upload_text_escapes_what_the_server_and_the_manifest_chose() {
    let summary = UploadSummary {
        remote_user: "ro\u{202e}ot".to_owned(),
        hostname: "pr\rod".to_owned(),
        remote_path: "/root/a\u{1b}b".to_owned(),
        creates_under: Some("/ro\u{202e}ot".to_owned()),
        server_id_tail: None,
        ..summary()
    };

    let body = upload_body(&summary);
    let request = ConfirmRequest::upload(PendingId::for_test(1), &summary);

    for text in [&body, &request.title] {
        assert!(
            !text.chars().any(|c| c.is_control() && c != '\n'),
            "{text:?}"
        );
        assert!(!text.contains('\u{202e}'), "{text:?}");
    }
    assert!(body.contains("inside /ro\\u{202e}ot:"));
}

#[test]
fn upload_body_warns_about_entries_that_anyone_can_write_and_places_that_run_code() {
    let summary = UploadSummary {
        risks: UploadRisks {
            world_writable: vec!["/srv/open.sh".to_owned()],
            runs_code: vec!["/etc/cron.d/job".to_owned()],
        },
        new_modes: BTreeMap::from([("/srv/open.sh".to_owned(), 0o666)]),
        ..summary()
    };

    let body = upload_body(&summary);

    assert!(body.contains(
        "WARNING: anyone on the server could change these new entries (mode allows write \
         for others):\n• /srv/open.sh (mode 0666)"
    ));
    assert!(body.contains(
        "WARNING: these new entries are where the server runs or trusts what it finds (cron, \
         sudoers, shell startup files, ssh keys, services, program directories):\n• /etc/cron.d/job"
    ));
}

#[test]
fn upload_body_has_no_risk_warning_when_there_is_nothing_to_warn_about() {
    assert!(!upload_body(&summary()).contains("WARNING"));
}

#[test]
fn upload_request_is_titled_with_the_real_remote_user() {
    let request = ConfirmRequest::upload(PendingId::for_test(1), &summary());

    assert_eq!(request.title, "Upload to root@prod-1?");
    assert_eq!(
        request.kind,
        ConfirmKind::Upload {
            id: PendingId::for_test(1)
        }
    );
}

#[test]
fn overwrite_request_lists_the_modified_files() {
    let request = ConfirmRequest::overwrite_local_changes(PendingId::for_test(2), &paths(2));

    assert_eq!(
        request.kind,
        ConfirmKind::OverwriteLocalChanges {
            id: PendingId::for_test(2)
        }
    );
    assert!(request.body.contains("• /etc/f0\n• /etc/f1"));
}

#[test]
fn upload_body_says_when_the_server_is_unchanged() {
    assert!(upload_body(&summary()).contains("unchanged since the last sync"));
}

#[test]
fn upload_body_warns_about_changes_made_on_the_server() {
    let summary = UploadSummary {
        remote_check: RemoteCheck::Checked(RemoteConflicts {
            changed: vec!["/etc/nginx/a.conf".to_owned()],
            missing: vec!["/etc/nginx/b.conf".to_owned()],
            already_exist: vec!["/etc/nginx/c.conf".to_owned()],
        }),
        ..summary()
    };

    let body = upload_body(&summary);

    assert!(body.contains("OVERWRITES these changes):\n• /etc/nginx/a.conf"));
    assert!(body.contains("unreadable (uploading recreates them):\n• /etc/nginx/b.conf"));
    assert!(body.contains("already on the server (uploading replaces them):\n• /etc/nginx/c.conf"));
    assert!(!body.contains("unchanged since the last sync"));
}

#[test]
fn upload_body_admits_when_the_server_could_not_be_checked() {
    let summary = UploadSummary {
        remote_check: RemoteCheck::Unavailable,
        ..summary()
    };

    let body = upload_body(&summary);

    assert!(body.contains("Could not check whether the server's files changed"));
    assert!(!body.contains("unchanged since the last sync"));
}

fn compare_summary() -> CompareSummary {
    CompareSummary {
        remote_user: "root".to_owned(),
        hostname: "prod-1".to_owned(),
        remote_path: "/etc/nginx".to_owned(),
        differences: vec![
            FileDifference {
                remote_path: "/etc/nginx/a.conf".to_owned(),
                change: FileChange::ChangedOnServer,
            },
            FileDifference {
                remote_path: "/etc/nginx/b.conf".to_owned(),
                change: FileChange::NewLocally,
            },
        ],
        identical_files: 5,
        diff_path: PathBuf::from("/mirror/.warp-sync/diffs/prod-1/etc_nginx.diff"),
        host_dir: PathBuf::from("/mirror/prod-1"),
        server_copy_dir: PathBuf::from("/mirror/.warp-sync/compare/prod-1"),
    }
}

fn vs_code() -> Option<EditorCli> {
    EditorCli::for_editor(Editor::VSCode)
}

#[test]
fn compare_result_lists_each_difference_with_who_changed_it() {
    let request = ConfirmRequest::compare_result(&compare_summary(), None);

    assert_eq!(request.title, "2 differences with root@prod-1");
    assert!(
        request
            .body
            .starts_with("prod-1:/etc/nginx\n\n2 files differ, 5 identical.")
    );
    assert!(
        request
            .body
            .contains("• changed on the server: /etc/nginx/a.conf")
    );
    assert!(request.body.contains("• new locally: /etc/nginx/b.conf"));
}

#[test]
fn compare_result_opens_the_diff_and_is_not_destructive() {
    let summary = compare_summary();

    let request = ConfirmRequest::compare_result(&summary, None);

    assert_eq!(
        request.kind,
        ConfirmKind::CompareResult {
            diff_path: summary.diff_path,
            editor_request: None,
        }
    );
    assert_eq!(request.kind.pending_id(), None);
    assert_eq!(request.confirm_label, "Open diff");
    assert_eq!(request.cancel_label, "Close");
    assert_eq!(request.style, ConfirmStyle::Neutral);
}

#[test]
fn compare_result_opens_side_by_side_in_the_external_editor() {
    let summary = compare_summary();

    let request = ConfirmRequest::compare_result(&summary, vs_code());

    assert_eq!(request.confirm_label, "Open in VS Code");
    assert!(request.body.contains("the server is on the left"));
    assert_eq!(
        request.kind,
        ConfirmKind::CompareResult {
            diff_path: summary.diff_path.clone(),
            editor_request: Some(summary.editor_request()),
        }
    );
}

#[test]
fn only_dialogs_that_resume_an_operation_have_a_pending_id() {
    let id = PendingId::for_test(7);

    assert_eq!(ConfirmKind::Upload { id }.pending_id(), Some(id));
    assert_eq!(
        ConfirmKind::OverwriteLocalChanges { id }.pending_id(),
        Some(id)
    );
}

#[test]
fn upload_and_overwrite_dialogs_are_destructive() {
    let overwrite = ConfirmRequest::overwrite_local_changes(PendingId::for_test(1), &paths(1));
    let upload = ConfirmRequest::upload(PendingId::for_test(2), &summary());

    assert_eq!(overwrite.style, ConfirmStyle::Destructive);
    assert_eq!(upload.style, ConfirmStyle::Destructive);
}

#[test]
fn control_characters_in_remote_paths_are_escaped() {
    let list = bullet_list(&["/etc/a\nb".to_owned()]);

    assert_eq!(list, "• /etc/a\\nb");
}
