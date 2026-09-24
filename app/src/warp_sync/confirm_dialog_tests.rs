use super::*;

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
        missing_locally: vec![],
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
fn upload_request_is_titled_with_the_real_remote_user() {
    let request = ConfirmRequest::upload(PendingId::for_test(1), &summary());

    assert_eq!(request.title, "Upload to root@prod-1?");
    assert_eq!(request.kind, ConfirmKind::Upload);
}

#[test]
fn overwrite_request_lists_the_modified_files() {
    let request = ConfirmRequest::overwrite_local_changes(PendingId::for_test(2), &paths(2));

    assert_eq!(request.kind, ConfirmKind::OverwriteLocalChanges);
    assert!(request.body.contains("• /etc/f0\n• /etc/f1"));
}
