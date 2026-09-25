use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use async_trait::async_trait;
use futures::channel::oneshot;
use warpui::ModelHandle;

use super::super::archive::UploadArchive;
use super::super::diff::FileChange;
use super::super::remote_script::{ProbeResult, ProbeStatus, RemoteKind, TarFlavor};
use super::super::transfer::UploadPlacement;
use super::*;
use crate::warp_sync::MAX_EXTERNAL_PENDING;

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
            expected_host_key: None,
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
                new_modes: BTreeMap::new(),
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
            placement: UploadPlacement::Replace,
            remote_check: RemoteCheck::Unavailable,
            remote_path: remote_path.to_owned(),
            host_key: host_key.to_owned(),
            mirror_root: PathBuf::from("/mirror"),
        },
        key: (host_key.to_owned(), remote_path.to_owned()),
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
        baseline_warning: None,
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
fn the_upload_message_mentions_a_baseline_that_could_not_be_recorded() {
    let outcome = UploadOutcome {
        baseline_warning: Some("Could not record the Git baseline: boom".to_owned()),
        ..upload_outcome(None)
    };

    let message = upload_message("/etc/nginx", &outcome);

    assert!(
        message.ends_with(". Could not record the Git baseline: boom"),
        "{message}"
    );
}

#[test]
fn the_upload_message_omits_the_backup_when_nothing_was_replaced() {
    let message = upload_message("/etc/nginx", &upload_outcome(None));

    assert!(!message.contains("saved to"));
}

fn compare_outcome(diff_path: Option<PathBuf>, differences: usize) -> CompareOutcome {
    CompareOutcome {
        differences: (0..differences)
            .map(|i| FileDifference {
                remote_path: format!("/etc/f{i}"),
                change: super::super::diff::FileChange::ChangedLocally,
            })
            .collect(),
        identical_files: 3,
        diff_path,
        remote_user: "root".to_owned(),
        host_dir: PathBuf::from("/m/h"),
        server_copy_dir: PathBuf::from("/m/.warp-sync/compare/h"),
    }
}

#[test]
fn a_comparison_without_differences_is_reported_as_a_success() {
    let event = compare_event(
        WindowId::new(),
        "prod-1".to_owned(),
        "/etc".to_owned(),
        compare_outcome(None, 0),
    );

    let WarpSyncEvent::Succeeded {
        message, location, ..
    } = event
    else {
        panic!("expected a success, got {event:?}");
    };
    assert_eq!(
        message,
        "No differences: /etc matches the local mirror (3 files)"
    );
    assert!(location.is_none());
}

#[test]
fn a_comparison_with_differences_carries_the_diff_location() {
    let diff_path = PathBuf::from("/mirror/.warp-sync/diffs/prod-1/etc.diff");

    let event = compare_event(
        WindowId::new(),
        "prod-1".to_owned(),
        "/etc".to_owned(),
        compare_outcome(Some(diff_path.clone()), 2),
    );

    let WarpSyncEvent::CompareFinished { summary, .. } = event else {
        panic!("expected a comparison, got {event:?}");
    };
    assert_eq!(summary.diff_path, diff_path);
    assert_eq!(summary.differences.len(), 2);
    assert_eq!(summary.identical_files, 3);
    assert_eq!(summary.hostname, "prod-1");
}

#[test]
fn the_announcement_names_the_path_and_the_diff_file() {
    let event = compare_event(
        WindowId::new(),
        "prod-1".to_owned(),
        "/etc".to_owned(),
        compare_outcome(Some(PathBuf::from("/m/etc.diff")), 1),
    );
    let WarpSyncEvent::CompareFinished { summary, .. } = event else {
        panic!("expected a comparison, got {event:?}");
    };

    assert_eq!(
        summary.announcement(),
        "Compared /etc: 1 difference. The diff is saved at /m/etc.diff"
    );
}

fn summary_with(changes: &[FileChange]) -> CompareSummary {
    CompareSummary {
        remote_user: "root".to_owned(),
        hostname: "prod-1".to_owned(),
        remote_path: "/etc".to_owned(),
        differences: changes
            .iter()
            .enumerate()
            .map(|(i, change)| FileDifference {
                remote_path: format!("/etc/f{i}"),
                change: *change,
            })
            .collect(),
        identical_files: 0,
        diff_path: PathBuf::from("/m/.warp-sync/diffs/h/etc.diff"),
        host_dir: PathBuf::from("/m/h"),
        server_copy_dir: PathBuf::from("/m/.warp-sync/compare/h"),
    }
}

#[test]
fn the_editor_shows_files_on_both_sides_next_to_each_other() {
    let summary = summary_with(&[FileChange::ChangedLocally, FileChange::ChangedOnServer]);

    assert_eq!(
        summary.editor_request(),
        EditorRequest::OpenDiffs {
            workspace: PathBuf::from("/m/h"),
            diffs: vec![
                (
                    PathBuf::from("/m/.warp-sync/compare/h/etc/f0"),
                    PathBuf::from("/m/h/etc/f0")
                ),
                (
                    PathBuf::from("/m/.warp-sync/compare/h/etc/f1"),
                    PathBuf::from("/m/h/etc/f1")
                ),
            ],
            files: Vec::new(),
        }
    );
}

#[test]
fn the_editor_also_gets_the_report_for_files_on_one_side() {
    let summary = summary_with(&[FileChange::NewLocally, FileChange::ChangedOnBoth]);

    let EditorRequest::OpenDiffs { diffs, files, .. } = summary.editor_request() else {
        panic!("expected diffs");
    };
    assert_eq!(diffs.len(), 1);
    assert_eq!(files, [PathBuf::from("/m/.warp-sync/diffs/h/etc.diff")]);
}

#[test]
fn the_editor_opens_a_bounded_number_of_diffs() {
    let summary = summary_with(&[FileChange::ChangedUnknown; MAX_EDITOR_DIFFS + 1]);

    let EditorRequest::OpenDiffs { diffs, files, .. } = summary.editor_request() else {
        panic!("expected diffs");
    };
    assert_eq!(diffs.len(), MAX_EDITOR_DIFFS);
    assert_eq!(files.len(), 1);
}

type CollectedEvents = Rc<RefCell<Vec<WarpSyncEvent>>>;

fn collect_events(app: &mut warpui::App, model: &ModelHandle<WarpSyncModel>) -> CollectedEvents {
    let events = CollectedEvents::default();
    let captured = events.clone();
    app.update(|ctx| {
        ctx.subscribe_to_model(model, move |_, event, _| {
            captured.borrow_mut().push(event.clone());
        });
    });
    events
}

fn reply_of(
    receiver: &mut oneshot::Receiver<Result<SyncReply, WarpSyncError>>,
) -> Result<SyncReply, WarpSyncError> {
    receiver
        .try_recv()
        .expect("the reply channel is open")
        .expect("a reply was sent")
}

/// Registers the path as in progress and hands an upload that is ready for confirmation to the
/// model, the way `start_upload` does once the remote host has been checked.
fn await_upload(app: &mut warpui::App, model: &ModelHandle<WarpSyncModel>, requester: Requester) {
    let PendingUpload {
        shell, prepared, ..
    } = pending_upload("prod-1", "/etc/nginx");
    model.update(app, |model, ctx| {
        model
            .try_begin_sync("prod-1", "/etc/nginx")
            .expect("path is free");
        let key = ("prod-1".to_owned(), "/etc/nginx".to_owned());
        model.await_upload_confirmation(shell, "prod-1".to_owned(), prepared, key, requester, ctx);
    });
}

fn sole_external_id(app: &warpui::App, model: &ModelHandle<WarpSyncModel>) -> Uuid {
    model.read(app, |model, _| {
        let mut ids = model.external_pending.keys().copied();
        let id = ids.next().expect("one external pending operation");
        assert!(ids.next().is_none());
        id
    })
}

#[test]
fn a_window_upload_waits_for_its_dialog_and_not_for_a_client() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let events = collect_events(&mut app, &model);
        let window_id = WindowId::new();

        await_upload(&mut app, &model, Requester::Window(window_id));

        model.read(&app, |model, _| {
            assert_eq!(model.pending_uploads.len(), 1);
            assert!(model.external_pending.is_empty());
        });
        assert!(matches!(
            events.borrow().as_slice(),
            [WarpSyncEvent::UploadNeedsConfirmation { window_id: event_window, .. }]
                if *event_window == window_id
        ));
    });
}

#[test]
fn an_external_upload_is_answered_with_a_reply_and_no_dialog() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let events = collect_events(&mut app, &model);
        let (requester, mut receiver) = Requester::external(WindowId::new(), None);

        await_upload(&mut app, &model, requester);

        let SyncReply::NeedsConfirmation { pending_id, kind } = reply_of(&mut receiver).unwrap()
        else {
            panic!("an upload must be confirmed before it is sent");
        };
        assert_eq!(pending_id, sole_external_id(&app, &model));
        let ConfirmationKind::Upload(summary) = kind else {
            panic!("expected an upload summary");
        };
        assert_eq!(summary.remote_user, "root");
        assert!(
            events.borrow().is_empty(),
            "no dialog for an external client"
        );
        model.read(&app, |model, _| assert!(model.pending_uploads.is_empty()));
    });
}

#[test]
fn a_held_external_operation_keeps_its_path_reserved() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let (requester, _receiver) = Requester::external(WindowId::new(), None);

        await_upload(&mut app, &model, requester);

        model.update(&mut app, |model, _| {
            assert_eq!(
                model.try_begin_sync("prod-1", "/etc/nginx"),
                Err(WarpSyncError::AlreadyInProgress)
            );
        });
    });
}

#[test]
fn external_ids_are_random_and_never_collide_with_window_ids() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let (requester, _receiver) = Requester::external(WindowId::new(), None);
        await_upload(&mut app, &model, requester);

        model.update(&mut app, |model, _| {
            model.cancel_pending(PendingId::for_test(1));
            assert_eq!(model.external_pending.len(), 1);
            assert_eq!(
                model.cancel_external(Uuid::new_v4()),
                Err(WarpSyncError::PendingNotFound)
            );
            assert_eq!(model.external_pending.len(), 1);
        });
    });
}

#[test]
fn a_window_pending_operation_cannot_be_confirmed_by_a_client() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        await_upload(&mut app, &model, Requester::Window(WindowId::new()));
        let window_pending_id = model.read(&app, |model, _| {
            *model
                .pending_uploads
                .keys()
                .next()
                .expect("one pending upload")
        });
        let (reply, mut receiver) = ExternalReply::channel();

        model.update(&mut app, |model, ctx| {
            model.confirm_external(Uuid::from_u128(u128::from(window_pending_id.0)), reply, ctx);
        });

        assert_eq!(
            reply_of(&mut receiver).unwrap_err(),
            WarpSyncError::PendingNotFound
        );
        model.read(&app, |model, _| assert_eq!(model.pending_uploads.len(), 1));
    });
}

#[test]
fn cancelling_an_external_operation_releases_its_path() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let (requester, _receiver) = Requester::external(WindowId::new(), None);
        await_upload(&mut app, &model, requester);
        let id = sole_external_id(&app, &model);

        model.update(&mut app, |model, _| {
            assert_eq!(model.cancel_external(id), Ok(()));
            assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());
            assert_eq!(
                model.cancel_external(id),
                Err(WarpSyncError::PendingNotFound),
                "an id can only be used once"
            );
        });
    });
}

#[test]
fn confirming_an_unknown_external_id_reports_that_nothing_is_pending() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let (reply, mut receiver) = ExternalReply::channel();

        model.update(&mut app, |model, ctx| {
            model.confirm_external(Uuid::new_v4(), reply, ctx);
        });

        assert_eq!(
            reply_of(&mut receiver).unwrap_err(),
            WarpSyncError::PendingNotFound
        );
    });
}

#[test]
fn an_external_operation_that_nobody_answers_expires_and_frees_its_path() {
    warpui::App::test((), |mut app| async move {
        let model =
            app.add_model(|_| WarpSyncModel::with_external_pending_ttl(Duration::from_millis(20)));
        let (requester, _receiver) = Requester::external(WindowId::new(), None);
        await_upload(&mut app, &model, requester);

        Timer::after(Duration::from_millis(300)).await;

        model.update(&mut app, |model, _| {
            assert!(model.external_pending.is_empty());
            assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());
        });
    });
}

#[test]
fn a_download_that_needs_confirmation_lists_the_files_for_a_client() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let (requester, mut receiver) = Requester::external(WindowId::new(), None);
        let PendingDownload { shell, request, .. } = pending_download("prod-1", "/etc/nginx");

        model.update(&mut app, |model, ctx| {
            model.await_download_confirmation(
                shell,
                request,
                vec!["/etc/nginx/a\nb".to_owned()],
                requester,
                ctx,
            );
        });

        let SyncReply::NeedsConfirmation { kind, .. } = reply_of(&mut receiver).unwrap() else {
            panic!("expected a confirmation request");
        };
        let ConfirmationKind::OverwriteLocalChanges { files } = kind else {
            panic!("expected an overwrite confirmation");
        };
        assert_eq!(files, vec!["/etc/nginx/a\\nb".to_owned()]);
    });
}

#[test]
fn an_external_operation_is_announced_in_the_window_of_its_session() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let events = collect_events(&mut app, &model);
        let window_id = WindowId::new();
        let (requester, _receiver) = Requester::external(window_id, None);

        model.update(&mut app, |_, ctx| {
            announce(&requester, "Uploading /etc…".to_owned(), ctx)
        });

        let events = events.borrow();
        let [
            WarpSyncEvent::Started {
                window_id: event_window,
                description,
            },
        ] = events.as_slice()
        else {
            panic!("expected a single Started event, got {events:?}");
        };
        assert_eq!(*event_window, window_id);
        assert_eq!(description, "Warp Sync (local control): Uploading /etc…");
    });
}

#[test]
fn failures_reach_a_window_as_events_and_a_client_as_an_error_reply() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let events = collect_events(&mut app, &model);
        let window_id = WindowId::new();
        let (external, mut receiver) = Requester::external(window_id, None);

        model.update(&mut app, |_, ctx| {
            report(
                Requester::Window(window_id),
                Finished::Failed(WarpSyncError::Timeout),
                ctx,
            );
            report(external, Finished::Failed(WarpSyncError::Timeout), ctx);
        });

        assert!(matches!(
            events.borrow().as_slice(),
            [WarpSyncEvent::Failed { window_id: event_window, error: WarpSyncError::Timeout }]
                if *event_window == window_id
        ));
        assert_eq!(reply_of(&mut receiver).unwrap_err(), WarpSyncError::Timeout);
    });
}

/// An upload whose mirror folder carries a machine suffix: the path was locked under the plain
/// host key before the machine was known.
const SUFFIXED_HOST_KEY: &str = "prod-1-ab12cd34";

fn suffixed_upload() -> PendingUpload {
    PendingUpload {
        key: ("prod-1".to_owned(), "/etc/nginx".to_owned()),
        ..pending_upload(SUFFIXED_HOST_KEY, "/etc/nginx")
    }
}

#[test]
fn cancelling_an_upload_to_a_suffixed_mirror_releases_the_path_that_was_locked() {
    let mut model = WarpSyncModel::new();
    model.try_begin_sync("prod-1", "/etc/nginx").unwrap();
    let id = model.next_pending_id();
    model.pending_uploads.insert(id, suffixed_upload());

    model.cancel_pending(id);

    assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());
}

#[test]
fn an_expired_external_upload_to_a_suffixed_mirror_releases_the_path_that_was_locked() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let (requester, _receiver) = Requester::external(WindowId::new(), None);
        let PendingUpload {
            shell, prepared, ..
        } = suffixed_upload();
        model.update(&mut app, |model, ctx| {
            model.try_begin_sync("prod-1", "/etc/nginx").unwrap();
            let key = ("prod-1".to_owned(), "/etc/nginx".to_owned());
            model.await_upload_confirmation(
                shell,
                "prod-1".to_owned(),
                prepared,
                key,
                requester,
                ctx,
            );
        });
        let id = sole_external_id(&app, &model);

        model.update(&mut app, |model, _| {
            assert_eq!(model.cancel_external(id), Ok(()));
            assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());
        });
    });
}

#[test]
fn a_confirmed_upload_to_a_suffixed_mirror_releases_the_path_when_it_finishes() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let events = collect_events(&mut app, &model);
        let window_id = WindowId::new();
        let PendingUpload {
            shell, prepared, ..
        } = suffixed_upload();
        model.update(&mut app, |model, ctx| {
            model.try_begin_sync("prod-1", "/etc/nginx").unwrap();
            let key = ("prod-1".to_owned(), "/etc/nginx".to_owned());
            model.await_upload_confirmation(
                shell,
                "prod-1".to_owned(),
                prepared,
                key,
                Requester::Window(window_id),
                ctx,
            );
        });
        let id = model.read(&app, |model, _| {
            *model
                .pending_uploads
                .keys()
                .next()
                .expect("one pending upload")
        });

        // The shell used here always times out, so the upload ends in a failure.
        model.update(&mut app, |model, ctx| model.confirm_upload(id, ctx));
        Timer::after(Duration::from_millis(300)).await;

        assert!(
            events
                .borrow()
                .iter()
                .any(|event| matches!(event, WarpSyncEvent::Failed { .. }))
        );
        model.update(&mut app, |model, _| {
            assert!(model.try_begin_sync("prod-1", "/etc/nginx").is_ok());
        });
    });
}

#[test]
fn too_many_operations_waiting_for_clients_are_refused_and_release_their_path() {
    warpui::App::test((), |mut app| async move {
        let model = app.add_model(|_| WarpSyncModel::new());
        let mut receivers = Vec::new();
        for index in 0..=MAX_EXTERNAL_PENDING {
            let path = format!("/etc/f{index}");
            let (requester, receiver) = Requester::external(WindowId::new(), None);
            let PendingUpload {
                shell, prepared, ..
            } = pending_upload("prod-1", &path);
            model.update(&mut app, |model, ctx| {
                model.try_begin_sync("prod-1", &path).unwrap();
                let key = ("prod-1".to_owned(), path.clone());
                model.await_upload_confirmation(
                    shell,
                    "prod-1".to_owned(),
                    prepared,
                    key,
                    requester,
                    ctx,
                );
            });
            receivers.push(receiver);
        }

        let (accepted, refused) = receivers.split_at_mut(MAX_EXTERNAL_PENDING);
        for receiver in accepted {
            assert!(matches!(
                reply_of(receiver),
                Ok(SyncReply::NeedsConfirmation { .. })
            ));
        }
        assert_eq!(
            reply_of(&mut refused[0]).unwrap_err(),
            WarpSyncError::TooManyPending
        );
        model.update(&mut app, |model, _| {
            assert_eq!(model.external_pending.len(), MAX_EXTERNAL_PENDING);
            let last = format!("/etc/f{MAX_EXTERNAL_PENDING}");
            assert!(
                model.try_begin_sync("prod-1", &last).is_ok(),
                "the refused path is free"
            );
        });
    });
}
