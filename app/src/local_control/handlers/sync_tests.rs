use std::fs;

use ::local_control::auth::CredentialRequest;
use ::local_control::protocol::{
    Action, ActionKind, SessionSelector, SessionTarget, SyncPathStatus, SyncResult, TargetSelector,
};
use ::local_control::{ControlError, ControlResponse, ErrorCode, InstanceId, RequestEnvelope, ResponseEnvelope};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, HOST};
use axum::http::{HeaderMap, HeaderValue};
use settings::Setting as _;
use warp_core::features::FeatureFlag;
use warpui::SingletonEntity as _;

use crate::local_control::{
    ControlServerState, LocalControlBridge, handle_control_request, issue_credential,
};
use crate::settings::{LocalControlMode, LocalControlSettings, WarpSyncSettings};
use crate::warp_sync::WarpSyncModel;

const EXPECTED_HOST: &str = "127.0.0.1:1234";

struct Harness {
    state: ControlServerState,
    mirror: tempfile::TempDir,
}

impl Harness {
    /// A running bridge whose Warp Sync mirror lives in a fresh temporary folder that already
    /// holds a folder for the host `prod-1`.
    async fn new(app: &mut warpui::App) -> Self {
        crate::test_util::settings::initialize_settings_for_tests(app);
        app.update(WarpSyncSettings::register);
        let mirror = tempfile::tempdir().expect("temp dir");
        fs::create_dir_all(mirror.path().join("prod-1/etc/nginx")).expect("host folder");
        let mirror_root = mirror.path().to_string_lossy().into_owned();
        app.update(|ctx| {
            LocalControlSettings::handle(ctx).update(ctx, |settings, ctx| {
                settings
                    .local_control_mode
                    .set_value(LocalControlMode::Enabled, ctx)
            })?;
            WarpSyncSettings::handle(ctx).update(ctx, |settings, ctx| {
                settings.mirror_root.set_value(mirror_root, ctx)
            })
        })
        .expect("settings apply");
        app.add_singleton_model(|_| WarpSyncModel::new());

        let instance_id = InstanceId("inst_test".to_owned());
        let bridge = app.add_singleton_model(LocalControlBridge::new);
        let state = bridge.update(app, |bridge, ctx| {
            bridge.set_instance_id(instance_id.clone());
            ControlServerState {
                bridge_spawner: ctx.spawner(),
                instance_id: instance_id.clone(),
                expected_host: EXPECTED_HOST.to_owned(),
                credentials: Default::default(),
            }
        });
        Self { state, mirror }
    }

    fn path(&self, relative: &str) -> String {
        self.mirror.path().join(relative).to_string_lossy().into_owned()
    }

    async fn call(
        &self,
        kind: ActionKind,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ControlError> {
        self.call_with_target(kind, params, TargetSelector::default())
            .await
    }

    async fn call_with_target(
        &self,
        kind: ActionKind,
        params: serde_json::Value,
        target: TargetSelector,
    ) -> Result<serde_json::Value, ControlError> {
        let credential = issue_credential(&self.state, CredentialRequest::new(kind)).await?;
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static(EXPECTED_HOST));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&credential.authorization_value()).expect("valid credential"),
        );
        let request = RequestEnvelope {
            target,
            ..RequestEnvelope::new(Action { kind, params })
        };
        let response = handle_control_request(
            State(self.state.clone()),
            headers,
            Bytes::from(serde_json::to_vec(&request).expect("request serializes")),
        )
        .await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let envelope = serde_json::from_slice::<ResponseEnvelope>(&body).expect("response decodes");
        match envelope.response {
            ControlResponse::Ok { data } => Ok(data),
            ControlResponse::Error { error } => Err(error),
        }
    }
}

fn enable_flags() -> (impl Sized, impl Sized) {
    (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::WarpSync.override_enabled(true),
    )
}

fn error_code(result: Result<serde_json::Value, ControlError>) -> ErrorCode {
    result.expect_err("the action should fail").code
}

#[test]
fn sync_actions_are_unsupported_when_warp_sync_is_off() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::WarpSync.override_enabled(false),
    );
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        for (kind, params) in [
            (ActionKind::SyncStatus, serde_json::json!({})),
            (ActionKind::SyncDownload, serde_json::json!({ "path": "/x" })),
            (ActionKind::SyncUploadPrepare, serde_json::json!({ "path": "/x" })),
            (ActionKind::SyncCompare, serde_json::json!({ "path": "/x" })),
            (
                ActionKind::SyncConfirm,
                serde_json::json!({ "pending_id": uuid::Uuid::new_v4() }),
            ),
            (
                ActionKind::SyncCancel,
                serde_json::json!({ "pending_id": uuid::Uuid::new_v4() }),
            ),
        ] {
            assert_eq!(
                error_code(harness.call(kind, params).await),
                ErrorCode::UnsupportedAction,
                "{}",
                kind.as_str()
            );
        }
    });
}

#[test]
fn status_reports_the_mirror_root() {
    let _flags = enable_flags();
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        let data = harness
            .call(ActionKind::SyncStatus, serde_json::json!({}))
            .await
            .expect("status succeeds");

        let result = serde_json::from_value::<SyncResult>(data).expect("status decodes");
        assert_eq!(
            result,
            SyncResult::Status {
                mirror_root: harness.mirror.path().to_string_lossy().into_owned(),
                path: None,
            }
        );
    });
}

#[test]
fn status_maps_a_mirror_path_to_its_host_and_remote_path() {
    let _flags = enable_flags();
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        let data = harness
            .call(
                ActionKind::SyncStatus,
                serde_json::json!({ "path": harness.path("prod-1/etc/nginx") }),
            )
            .await
            .expect("status succeeds");

        let SyncResult::Status { path, .. } =
            serde_json::from_value::<SyncResult>(data).expect("status decodes")
        else {
            panic!("expected a status result");
        };
        assert_eq!(
            path,
            Some(SyncPathStatus {
                host_key: "prod-1".to_owned(),
                remote_path: Some("/etc/nginx".to_owned()),
                sessions: Vec::new(),
            })
        );
    });
}

#[test]
fn paths_outside_the_mirror_are_rejected_before_any_session_is_looked_at() {
    let _flags = enable_flags();
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;
        let outside = tempfile::tempdir().expect("temp dir");
        let outside = outside.path().to_string_lossy().into_owned();

        for kind in [
            ActionKind::SyncDownload,
            ActionKind::SyncUploadPrepare,
            ActionKind::SyncCompare,
        ] {
            for path in [
                outside.clone(),
                "/etc/passwd".to_owned(),
                "relative/path".to_owned(),
                harness.path("prod-1/../../etc"),
                harness.path(".warp-sync/compare"),
                harness.path("prod-1/.git/config"),
            ] {
                assert_eq!(
                    error_code(harness.call(kind, serde_json::json!({ "path": path })).await),
                    ErrorCode::InvalidParams,
                    "{} {path}",
                    kind.as_str()
                );
            }
        }
    });
}

#[test]
fn the_host_folder_itself_cannot_be_synced() {
    let _flags = enable_flags();
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        for kind in [
            ActionKind::SyncDownload,
            ActionKind::SyncUploadPrepare,
            ActionKind::SyncCompare,
        ] {
            assert_eq!(
                error_code(
                    harness
                        .call(kind, serde_json::json!({ "path": harness.path("prod-1") }))
                        .await
                ),
                ErrorCode::InvalidParams,
                "{}",
                kind.as_str()
            );
        }
    });
}

#[test]
fn a_valid_path_without_an_open_session_asks_for_one() {
    let _flags = enable_flags();
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        for kind in [
            ActionKind::SyncDownload,
            ActionKind::SyncUploadPrepare,
            ActionKind::SyncCompare,
        ] {
            let error = harness
                .call(
                    kind,
                    serde_json::json!({ "path": harness.path("prod-1/etc/nginx/nginx.conf") }),
                )
                .await
                .expect_err("no session is open");
            assert_eq!(error.code, ErrorCode::MissingTarget, "{}: {error}", kind.as_str());
            assert!(error.message.contains("prod-1"), "{error}");
        }
    });
}

#[test]
fn an_explicit_session_that_does_not_exist_is_rejected() {
    let _flags = enable_flags();
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;
        let target = TargetSelector {
            session: Some(SessionTarget::Id {
                id: SessionSelector("no-such-session".to_owned()),
            }),
            ..TargetSelector::default()
        };

        let error = harness
            .call_with_target(
                ActionKind::SyncDownload,
                serde_json::json!({ "path": harness.path("prod-1/etc/nginx") }),
                target,
            )
            .await
            .expect_err("the session is unknown");

        assert_eq!(error.code, ErrorCode::StaleTarget, "{error}");
    });
}

#[test]
fn confirming_or_cancelling_something_that_is_not_pending_is_reported() {
    let _flags = enable_flags();
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;
        let params = serde_json::json!({ "pending_id": uuid::Uuid::new_v4() });

        assert_eq!(
            error_code(harness.call(ActionKind::SyncConfirm, params.clone()).await),
            ErrorCode::StaleTarget
        );
        assert_eq!(
            error_code(harness.call(ActionKind::SyncCancel, params).await),
            ErrorCode::StaleTarget
        );
    });
}

#[test]
fn a_malformed_pending_id_is_rejected_as_invalid_params() {
    let _flags = enable_flags();
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        for kind in [ActionKind::SyncConfirm, ActionKind::SyncCancel] {
            assert_eq!(
                error_code(
                    harness
                        .call(kind, serde_json::json!({ "pending_id": "7" }))
                        .await
                ),
                ErrorCode::InvalidParams,
                "{}",
                kind.as_str()
            );
        }
    });
}
