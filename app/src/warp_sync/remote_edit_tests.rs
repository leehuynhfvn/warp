use super::*;

fn file() -> RemoteEditFile {
    RemoteEditFile::new(
        SessionId::from(7),
        WindowId::new(),
        "/etc/nginx/nginx.conf".to_owned(),
        "root".to_owned(),
        "draff3".to_owned(),
    )
}

#[test]
fn a_manual_save_requests_an_upload() {
    let mut file = file();

    assert!(file.on_saved(false));
    assert_eq!(file.state(), &RemoteEditState::Uploading);
}

#[test]
fn an_auto_save_never_uploads() {
    let mut file = file();

    assert!(!file.on_saved(true));
    assert_eq!(file.state(), &RemoteEditState::Unsynced);
}

#[test]
fn a_save_during_an_upload_does_not_start_another_one() {
    let mut file = file();
    file.on_saved(false);

    assert!(!file.on_saved(false));
    assert_eq!(file.state(), &RemoteEditState::Uploading);
}

#[test]
fn a_finished_upload_leaves_the_file_clean() {
    let mut file = file();
    file.on_saved(false);

    file.on_upload_finished(UploadResult::Uploaded);

    assert_eq!(file.state(), &RemoteEditState::Clean);
}

#[test]
fn edits_saved_during_an_upload_stay_unsynced() {
    let mut file = file();
    file.on_saved(false);
    file.on_saved(true);

    file.on_upload_finished(UploadResult::Uploaded);

    assert_eq!(file.state(), &RemoteEditState::Unsynced);

    file.on_saved(false);
    file.on_upload_finished(UploadResult::Uploaded);
    assert_eq!(file.state(), &RemoteEditState::Clean);
}

#[test]
fn a_cancelled_upload_leaves_the_file_unsynced_and_a_later_save_asks_again() {
    let mut file = file();
    file.on_saved(false);

    file.on_upload_finished(UploadResult::Cancelled);

    assert_eq!(file.state(), &RemoteEditState::Unsynced);
    assert!(file.on_saved(false));
}

#[test]
fn a_failed_upload_keeps_its_reason_and_can_be_retried() {
    let mut file = file();
    file.on_saved(false);

    file.on_upload_finished(UploadResult::Failed("session ended".to_owned()));

    assert_eq!(
        file.state(),
        &RemoteEditState::Failed("session ended".to_owned())
    );
    assert!(file.on_saved(false));
}

#[test]
fn the_status_line_names_the_server_path_and_state() {
    let mut file = file();

    assert_eq!(
        file.status_line(),
        "root@draff3:/etc/nginx/nginx.conf · Save uploads to the server"
    );
    file.on_saved(true);
    assert_eq!(
        file.status_line(),
        "root@draff3:/etc/nginx/nginx.conf · Unsynced changes"
    );
}

#[test]
fn the_status_line_does_not_print_control_characters_from_the_server() {
    let file = RemoteEditFile::new(
        SessionId::from(7),
        WindowId::new(),
        "/tmp/a\u{1b}[31mb".to_owned(),
        "root".to_owned(),
        "draff3".to_owned(),
    );

    assert!(!file.status_line().contains('\u{1b}'));
}

#[test]
fn absolute_and_path_like_relative_words_become_remote_links() {
    let cases = [
        (
            "/etc/nginx/nginx.conf",
            "/root",
            Some("/etc/nginx/nginx.conf"),
        ),
        ("nginx.conf", "/etc/nginx", Some("/etc/nginx/nginx.conf")),
        (
            "conf.d/site.conf",
            "/etc/nginx",
            Some("/etc/nginx/conf.d/site.conf"),
        ),
        (
            "sites-enabled/",
            "/etc/nginx",
            Some("/etc/nginx/sites-enabled"),
        ),
        (".bashrc", "/root", Some("/root/.bashrc")),
    ];
    for (token, pwd, expected) in cases {
        assert_eq!(
            remote_link_path(token, pwd).as_deref(),
            expected,
            "{token} in {pwd}"
        );
    }
}

#[test]
fn words_that_do_not_look_like_paths_are_not_links() {
    for token in [
        "",
        "root",
        "4096",
        "1.2.3",
        "192.168.1.10",
        "-rw-r--r--",
        "--config=/etc/x.conf",
        "https://example.com/a.conf",
        "/",
        "a b.conf",
        "..",
        "../hosts",
        "a\u{1b}.conf",
    ] {
        assert_eq!(remote_link_path(token, "/etc"), None, "{token:?}");
    }
}
