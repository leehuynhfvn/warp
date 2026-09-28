use std::time::{Duration, SystemTime};

use ::local_control::auth::CredentialRequest;
use ::local_control::protocol::{Action, ActionKind, AgentToken};
use ::local_control::{
    ControlError, ControlResponse, ErrorCode, InstanceId, RequestEnvelope, ResponseEnvelope,
};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, HOST};
use axum::http::{HeaderMap, HeaderValue};
use settings::Setting as _;
use uuid::Uuid;
use warp_core::features::FeatureFlag;
use warpui::{SingletonEntity as _, WindowId};

use crate::agent_bridge::approval::{AgentLabel, ApprovalRequest, ApprovalSubject};
use crate::agent_bridge::model::AgentBridgeModel;
use crate::agent_bridge::pairing;
use crate::local_control::{
    ControlServerState, LocalControlBridge, handle_control_request, issue_credential,
};
use crate::settings::{LocalControlMode, LocalControlSettings};

const EXPECTED_HOST: &str = "127.0.0.1:1234";

struct Harness {
    state: ControlServerState,
}

impl Harness {
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
        name: &str,
        token: Option<&AgentToken>,
    ) -> Result<serde_json::Value, ControlError> {
        let kind = ActionKind::AgentPair;
        let credential = issue_credential(&self.state, CredentialRequest::new(kind))
            .await
            .expect("credential issues");
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static(EXPECTED_HOST));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&credential.authorization_value()).expect("valid credential"),
        );
        let request = RequestEnvelope {
            agent_token: token.cloned(),
            ..RequestEnvelope::new(Action {
                kind,
                params: serde_json::json!({ "name": name }),
            })
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

/// Points `$HOME` at a temporary, empty directory, runs `body`, then restores `$HOME`. `#[serial]`
/// on the caller keeps this from racing another test's `$HOME` (shared with the policy tests in
/// `remote_tests.rs`, which mutate the same environment variable).
fn with_temp_home(body: impl FnOnce(&std::path::Path)) {
    let home = tempfile::tempdir().unwrap();
    let previous_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    body(home.path());
    unsafe {
        match &previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[test]
#[serial_test::serial]
fn a_request_without_a_token_is_rejected() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    with_temp_home(|_home| {
        warpui::App::test((), |mut app| async move {
            let harness = Harness::new(&mut app).await;
            let error = harness.call("claude-code", None).await.unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidParams);
            assert!(error.message.contains("agent_token"), "{error:?}");
        });
    });
}

#[test]
#[serial_test::serial]
fn an_already_paired_token_answers_immediately() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    with_temp_home(|home| {
        let token = AgentToken::generate();
        pairing::add(home, "claude-code", pairing::hash(token.secret())).unwrap();
        warpui::App::test((), |mut app| async move {
            let harness = Harness::new(&mut app).await;
            let data = harness.call("claude-code", Some(&token)).await.unwrap();
            assert_eq!(data["status"], "already_paired");
            assert_eq!(data["agent_id"], "claude-code");
        });
    });
}

#[test]
#[serial_test::serial]
fn a_second_pairing_request_is_refused_while_one_is_already_waiting() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    with_temp_home(|_home| {
        warpui::App::test((), |mut app| async move {
            let harness = Harness::new(&mut app).await;

            app.update(|ctx| {
                AgentBridgeModel::handle(ctx).update(ctx, |model, ctx| {
                    let _ = model.push_approval(
                        ApprovalRequest {
                            request_id: Uuid::new_v4(),
                            session: None,
                            session_label: String::new(),
                            agent: AgentLabel {
                                claimed: Some("agent-a".to_owned()),
                                agent_id: None,
                            },
                            subject: ApprovalSubject::Pairing {
                                name: "agent-a".to_owned(),
                            },
                            deadline: SystemTime::now() + Duration::from_secs(300),
                            window_id: WindowId::from_usize(1),
                        },
                        ctx,
                    );
                })
            });

            let token = AgentToken::generate();
            let error = harness.call("agent-b", Some(&token)).await.unwrap_err();
            assert_eq!(error.code, ErrorCode::SessionBusy);
            assert!(error.message.contains("waiting to be paired"), "{error:?}");
        });
    });
}
