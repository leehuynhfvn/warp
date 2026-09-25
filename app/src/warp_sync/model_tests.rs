use std::path::PathBuf;

use async_trait::async_trait;

use super::super::archive::UploadArchive;
use super::super::remote_script::{ProbeResult, ProbeStatus, RemoteKind, TarFlavor};
use super::*;

struct NeverShell;

#[async_trait]
impl RemoteShell for NeverShell {
    async fn run(&self, _command: &str) -> Result<Vec<u8>, WarpSyncError> {
        Err(WarpSyncError::Timeout)
    }
}

fn pending_download(host_key: &str, remote_path: &str) -> PendingDownload {
    PendingDownload {
        shell: Arc::new(NeverShell),
        request: DownloadRequest {
            remote_path: remote_path.to_owned(),
            host_key: host_key.to_owned(),
            mirror_root: PathBuf::from("/mirror"),
            limits: SyncLimits::default(),
            allow_overwrite_local_changes: false,
        },
        window_id: WindowId::new(),
    }
}

fn pending_upload(host_key: &str, remote_path: &str) -> PendingUpload {
    PendingUpload {
        shell: Arc::new(NeverShell),
        prepared: PreparedUpload {
            archive: UploadArchive {
                bytes: Vec::new(),
                files: 0,
                dirs: 0,
                content_bytes: 0,
                new_files: Vec::new(),
                missing_locally: Vec::new(),
            },
            probe: ProbeResult {
                status: ProbeStatus::Ok,
                user: "root".to_owned(),
                uid: 0,
                kind: RemoteKind::Dir,
                size_kib: None,
                tar: TarFlavor::Gnu,
                has_base64: true,
                machine_id: Some("0123456789abcdef".to_owned()),
            },
            remote_check: RemoteCheck::Unavailable,
            remote_path: remote_path.to_owned(),
            host_key: host_key.to_owned(),
            mirror_root: PathBuf::from("/mirror"),
        },
        window_id: WindowId::new(),
    }
}

#[test]
fn the_same_path_on_the_same_host_cannot_be_synced_twice_at_once() {
    let mut model = WarpSyncModel::new();

    assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());

    assert_eq!(
        model.try_begin_sync("prod-1", "/etc/nginx"),
        Err(WarpSyncError::AlreadyInProgress)
    );
}

#[test]
fn a_path_conflicts_with_its_ancestors_and_descendants() {
    let mut model = WarpSyncModel::new();
    model.try_begin_sync("prod-1", "/etc/nginx").unwrap();

    assert_eq!(
        model.try_begin_sync("prod-1", "/etc"),
        Err(WarpSyncError::AlreadyInProgress)
    );
    assert_eq!(
        model.try_begin_sync("prod-1", "/etc/nginx/conf.d"),
        Err(WarpSyncError::AlreadyInProgress)
    );
}

#[test]
fn unrelated_paths_and_other_hosts_can_be_synced_concurrently() {
    let mut model = WarpSyncModel::new();
    model.try_begin_sync("prod-1", "/etc/nginx").unwrap();

    assert!(model.try_begin_sync("prod-1", "/etc/nginx2").is_ok());
    assert!(model.try_begin_sync("prod-1", "/var/log").is_ok());
    assert!(model.try_begin_sync("prod-2", "/etc/nginx").is_ok());
}

#[test]
fn a_finished_sync_can_be_started_again() {
    let mut model = WarpSyncModel::new();
    model.try_begin_sync("prod-1", "/etc/nginx").unwrap();

    model.finish_sync(&("prod-1".to_owned(), "/etc/nginx".to_owned()));

    assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());
}

#[test]
fn pending_ids_are_unique() {
    let mut model = WarpSyncModel::new();

    let first = model.next_pending_id();
    let second = model.next_pending_id();

    assert_ne!(first, second);
}

#[test]
fn cancelling_a_pending_download_releases_its_path() {
    let mut model = WarpSyncModel::new();
    model.try_begin_sync("prod-1", "/etc/nginx").unwrap();
    let id = model.next_pending_id();
    model
        .pending_downloads
        .insert(id, pending_download("prod-1", "/etc/nginx"));

    model.cancel_pending(id);

    assert!(model.pending_downloads.is_empty());
    assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());
}

#[test]
fn cancelling_a_pending_upload_releases_its_path() {
    let mut model = WarpSyncModel::new();
    model.try_begin_sync("prod-1", "/etc/nginx").unwrap();
    let id = model.next_pending_id();
    model
        .pending_uploads
        .insert(id, pending_upload("prod-1", "/etc/nginx"));

    model.cancel_pending(id);

    assert!(model.pending_uploads.is_empty());
    assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());
}

#[test]
fn a_path_waiting_for_confirmation_stays_reserved() {
    let mut model = WarpSyncModel::new();
    model.try_begin_sync("prod-1", "/etc/nginx").unwrap();
    let id = model.next_pending_id();
    model
        .pending_uploads
        .insert(id, pending_upload("prod-1", "/etc/nginx"));

    assert_eq!(
        model.try_begin_sync("prod-1", "/etc/nginx"),
        Err(WarpSyncError::AlreadyInProgress)
    );
}

#[test]
fn cancelling_an_unknown_id_changes_nothing() {
    let mut model = WarpSyncModel::new();
    model.try_begin_sync("prod-1", "/etc/nginx").unwrap();
    let unknown = model.next_pending_id();

    model.cancel_pending(unknown);

    assert_eq!(
        model.try_begin_sync("prod-1", "/etc/nginx"),
        Err(WarpSyncError::AlreadyInProgress)
    );
}

#[test]
fn overlap_compares_whole_path_components() {
    assert!(paths_overlap("/etc", "/etc"));
    assert!(paths_overlap("/etc", "/etc/nginx"));
    assert!(paths_overlap("/etc/nginx", "/etc"));
    assert!(!paths_overlap("/etc/nginx", "/etc/nginx2"));
    assert!(!paths_overlap("/etc/nginx2", "/etc/nginx"));
}

#[test]
fn sizes_are_formatted_with_a_sensible_unit() {
    assert_eq!(format_size(512), "512 B");
    assert_eq!(format_size(1536), "1.5 KiB");
    assert_eq!(format_size(5 * 1024 * 1024), "5.0 MiB");
}

#[test]
fn counts_are_pluralized() {
    assert_eq!(pluralize_count(1, "file"), "1 file");
    assert_eq!(pluralize_count(0, "file"), "0 files");
    assert_eq!(pluralize_count(12, "folder"), "12 folders");
}

fn upload_outcome(backup_path: Option<&str>) -> UploadOutcome {
    UploadOutcome {
        files: 1,
        dirs: 2,
        content_bytes: 2048,
        backup_path: backup_path.map(str::to_owned),
        remote_user: "root".to_owned(),
    }
}

#[test]
fn the_upload_message_reports_the_remote_user_and_the_backup() {
    let message = upload_message(
        "/etc/nginx",
        &upload_outcome(Some("/root/.warp-sync/b.tgz")),
    );

    assert_eq!(
        message,
        "Uploaded 1 file and 2 folders (2.0 KiB) to /etc/nginx as root. \
         Previous version saved to /root/.warp-sync/b.tgz"
    );
}

#[test]
fn the_upload_message_omits_the_backup_when_nothing_was_replaced() {
    let message = upload_message("/etc/nginx", &upload_outcome(None));

    assert!(!message.contains("saved to"));
}
