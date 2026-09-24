use super::*;

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
fn different_paths_and_hosts_can_be_synced_concurrently() {
    let mut model = WarpSyncModel::new();

    assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());

    assert!(model.try_begin_sync("prod-1", "/etc/nginx2").is_ok());
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
fn cancelling_an_unknown_pending_operation_is_a_no_op() {
    let mut model = WarpSyncModel::new();
    let id = model.next_pending_id();

    model.cancel_pending(id);

    assert!(model.pending_downloads.is_empty());
    assert!(model.pending_uploads.is_empty());
}

#[test]
fn sizes_are_formatted_with_a_sensible_unit() {
    assert_eq!(format_size(512), "512 B");
    assert_eq!(format_size(1536), "1.5 KiB");
    assert_eq!(format_size(5 * 1024 * 1024), "5.0 MiB");
}

#[test]
fn file_counts_are_pluralized() {
    assert_eq!(pluralize_files(1), "1 file");
    assert_eq!(pluralize_files(0), "0 files");
    assert_eq!(pluralize_files(12), "12 files");
}
