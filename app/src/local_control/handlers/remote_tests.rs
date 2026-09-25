use ::local_control::auth::CredentialRequest;
use ::local_control::protocol::{
    Action, ActionKind, SessionSelector, SessionTarget, TargetSelector, WriteExpectation,
};
use ::local_control::{
    ControlError, ControlResponse, ErrorCode, InstanceId, RequestEnvelope, ResponseEnvelope,
};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, HOST};
use axum::http::{HeaderMap, HeaderValue};
use settings::Setting as _;
use warp_core::features::FeatureFlag;
use warpui::SingletonEntity as _;

use crate::agent_bridge::model::AgentBridgeModel;
use crate::local_control::{
    ControlServerState, LocalControlBridge, handle_control_request, issue_credential,
};
use crate::settings::{LocalControlMode, LocalControlSettings};

const EXPECTED_HOST: &str = "127.0.0.1:1234";

struct Harness {
    state: ControlServerState,
}

impl Harness {
    /// A running bridge that accepts requests, in an app without any window.
    async fn new(app: &mut warpui::App) -> Self {
        crate::test_util::settings::initialize_settings_for_tests(app);
        app.update(|ctx| {
            LocalControlSettings::handle(ctx).update(ctx, |settings, ctx| {
                settings
                    .local_control_mode
                    .set_value(LocalControlMode::Enabled, ctx)
            })
        })
        .expect("settings apply");
        app.add_singleton_model(|_| AgentBridgeModel::default());

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
        Self { state }
    }

    async fn call(
        &self,
        kind: ActionKind,
        params: serde_json::Value,
        session: Option<SessionTarget>,
    ) -> Result<serde_json::Value, ControlError> {
        let credential = issue_credential(&self.state, CredentialRequest::new(kind)).await?;
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static(EXPECTED_HOST));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&credential.authorization_value()).expect("valid credential"),
        );
        let request = RequestEnvelope {
            target: TargetSelector {
                session,
                ..TargetSelector::default()
            },
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

fn exec_params() -> serde_json::Value {
    serde_json::json!({ "command": "id" })
}

fn read_params() -> serde_json::Value {
    serde_json::json!({ "path": "/etc/hosts" })
}

fn write_params() -> serde_json::Value {
    serde_json::json!({
        "path": "/etc/hosts",
        "content_base64": "",
        "expectation": WriteExpectation::MustNotExist,
    })
}

fn session_id(id: &str) -> Option<SessionTarget> {
    Some(SessionTarget::Id {
        id: SessionSelector(id.to_owned()),
    })
}

fn error_code(result: Result<serde_json::Value, ControlError>) -> ErrorCode {
    result.expect_err("the action should fail").code
}

const SERVER_ACTIONS: [ActionKind; 3] = [
    ActionKind::RemoteExec,
    ActionKind::RemoteFileRead,
    ActionKind::RemoteFileWrite,
];

fn params_for(kind: ActionKind) -> serde_json::Value {
    match kind {
        ActionKind::RemoteExec => exec_params(),
        ActionKind::RemoteFileRead => read_params(),
        _ => write_params(),
    }
}

#[test]
fn remote_actions_are_unsupported_when_the_bridge_is_off() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(false),
    );
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        for kind in SERVER_ACTIONS {
            let result = harness.call(kind, params_for(kind), session_id("1")).await;
            assert_eq!(error_code(result), ErrorCode::UnsupportedAction, "{kind:?}");
        }
        let list = harness
            .call(ActionKind::RemoteSessionList, serde_json::json!({}), None)
            .await;
        assert_eq!(error_code(list), ErrorCode::UnsupportedAction);
    });
}

#[test]
fn a_request_must_name_a_session_explicitly() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        for kind in SERVER_ACTIONS {
            for session in [None, Some(SessionTarget::Active)] {
                let result = harness.call(kind, params_for(kind), session.clone()).await;
                assert_eq!(
                    error_code(result),
                    ErrorCode::MissingTarget,
                    "{kind:?} with {session:?}"
                );
            }
        }
    });
}

#[test]
fn a_session_that_does_not_exist_is_a_stale_target() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        for kind in SERVER_ACTIONS {
            let result = harness.call(kind, params_for(kind), session_id("999")).await;
            assert_eq!(error_code(result), ErrorCode::StaleTarget, "{kind:?}");
        }
    });
}

#[test]
fn malformed_params_are_rejected_before_any_session_is_looked_up() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        let unknown_field = serde_json::json!({ "command": "id", "shell": "zsh" });
        let result = harness
            .call(ActionKind::RemoteExec, unknown_field, session_id("1"))
            .await;
        assert_eq!(error_code(result), ErrorCode::InvalidParams);

        let no_expectation = serde_json::json!({ "path": "/x", "content_base64": "" });
        let result = harness
            .call(ActionKind::RemoteFileWrite, no_expectation, session_id("1"))
            .await;
        assert_eq!(error_code(result), ErrorCode::InvalidParams);
    });
}

#[test]
fn the_session_list_is_empty_without_windows_and_is_not_an_error() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        let result = harness
            .call(ActionKind::RemoteSessionList, serde_json::json!({}), None)
            .await;
        assert_eq!(
            result.expect("an empty list is not an error"),
            serde_json::json!({ "sessions": [] })
        );
    });
}

#[test]
fn output_recent_is_not_implemented_yet() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;
        let result = harness
            .call(
                ActionKind::RemoteOutputRecent,
                serde_json::json!({}),
                session_id("1"),
            )
            .await;
        assert_eq!(error_code(result), ErrorCode::UnsupportedAction);
    });
}
