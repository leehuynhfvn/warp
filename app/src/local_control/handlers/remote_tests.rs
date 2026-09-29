use ::local_control::auth::CredentialRequest;
use ::local_control::protocol::{
    Action, ActionKind, RemoteExecParams, RemoteFileReadParams, RemoteFileWriteParams,
    SessionSelector, SessionTarget, TargetSelector, WriteExpectation,
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

use super::{Decision, Operation, PolicyRequest, PolicySubject, evaluate_policy, resolve_agent_id};
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

const SESSION_ACTIONS: [ActionKind; 5] = [
    ActionKind::RemoteExec,
    ActionKind::RemoteExecVisible,
    ActionKind::RemoteFileRead,
    ActionKind::RemoteFileWrite,
    ActionKind::RemoteOutputRecent,
];

fn params_for(kind: ActionKind) -> serde_json::Value {
    match kind {
        ActionKind::RemoteExec | ActionKind::RemoteExecVisible => exec_params(),
        ActionKind::RemoteFileRead => read_params(),
        ActionKind::RemoteOutputRecent => serde_json::json!({ "count": 2 }),
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

        for kind in SESSION_ACTIONS {
            let result = harness.call(kind, params_for(kind), session_id("1")).await;
            assert_eq!(error_code(result), ErrorCode::UnsupportedAction, "{kind:?}");
        }
        let list = harness
            .call(ActionKind::RemoteSessionList, serde_json::json!({}), None)
            .await;
        assert_eq!(error_code(list), ErrorCode::UnsupportedAction);
        let hosts = harness
            .call(ActionKind::RemoteHostList, serde_json::json!({}), None)
            .await;
        assert_eq!(error_code(hosts), ErrorCode::UnsupportedAction);
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

        for kind in SESSION_ACTIONS {
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

        for kind in SESSION_ACTIONS {
            let result = harness
                .call(kind, params_for(kind), session_id("999"))
                .await;
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
fn output_recent_rejects_a_count_out_of_range_and_a_bad_agent_name() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        for params in [
            serde_json::json!({ "count": 0 }),
            serde_json::json!({ "count": 11 }),
            serde_json::json!({ "agent": "Gemini CLI" }),
        ] {
            let result = harness
                .call(
                    ActionKind::RemoteOutputRecent,
                    params.clone(),
                    session_id("1"),
                )
                .await;
            assert_eq!(error_code(result), ErrorCode::InvalidParams, "{params}");
        }
    });
}

// --- Agent-ops policy wiring ------------------------------------------------------------------
//
// `remote_tests.rs`'s HTTP harness never attaches a real `WarpifiedRemote` session (every test
// above stops at `StaleTarget`/`InvalidParams`/before `resolve` even runs), so `authorize` cannot
// be exercised end to end here. `Policy::evaluate` itself has full coverage in
// `agent_bridge::policy_tests`; these tests cover the two things that live in this module: which
// operations the policy applies to, and that `evaluate_policy` resolves the real policy file
// under `$HOME`.

#[test]
fn only_exec_and_write_operations_are_subject_to_the_agent_ops_policy() {
    let exec = Operation::Exec(RemoteExecParams {
        command: "id -un".to_owned(),
        cwd: None,
        timeout_secs: None,
        agent: None,
    });
    match exec.policy_subject() {
        Some(PolicySubject::Exec(command)) => assert_eq!(command, "id -un"),
        other => panic!("expected Some(Exec), got {other:?}"),
    }

    let read = Operation::Read(RemoteFileReadParams {
        path: "/etc/hosts".to_owned(),
        agent: None,
    });
    assert!(
        read.policy_subject().is_none(),
        "a read must not be subject to the agent-ops policy (L0 in mục 1.1 of the plan)"
    );

    let write = Operation::Write(RemoteFileWriteParams {
        path: "/etc/hosts".to_owned(),
        content_base64: String::new(),
        expectation: WriteExpectation::MustNotExist,
        agent: None,
    });
    match write.policy_subject() {
        Some(PolicySubject::Write { path, creates, .. }) => {
            assert_eq!(path, "/etc/hosts");
            assert!(creates, "MustNotExist means the write creates a new file");
        }
        other => panic!("expected Some(Write), got {other:?}"),
    }
}

/// Points `$HOME` at a temporary directory holding `.warp/agent-ops/policy.toml`, runs `body`,
/// then restores `$HOME`. `#[serial]` on the caller keeps this from racing another test's `$HOME`.
fn with_temp_home_policy(text: &str, body: impl FnOnce()) {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".warp").join("agent-ops");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.toml");
    std::fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let previous_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home.path());
    }
    body();
    unsafe {
        match &previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[test]
#[serial_test::serial]
fn evaluate_policy_reads_the_real_policy_file_under_home() {
    with_temp_home_policy("[defaults]\nmode = \"read_only\"\n", || {
        let decision = evaluate_policy(
            dirs::home_dir().as_deref(),
            "prod-1",
            PolicyRequest::Exec("id"),
            false,
        );
        match decision {
            Decision::Deny(reason) => assert!(reason.contains("read-only"), "{reason}"),
            other => panic!("expected Deny, got {other:?}"),
        }
    });
}

#[test]
#[serial_test::serial]
fn evaluate_policy_denies_everything_when_the_policy_file_is_invalid() {
    with_temp_home_policy("[defaults]\nmode = \"sometimes\"\n", || {
        let decision = evaluate_policy(
            dirs::home_dir().as_deref(),
            "prod-1",
            PolicyRequest::Exec("id"),
            false,
        );
        match decision {
            Decision::Deny(reason) => {
                assert!(reason.contains("policy file"), "{reason}");
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    });
}

#[test]
fn resolve_agent_id_is_none_without_a_token() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(resolve_agent_id(home.path(), None), None);
}

#[test]
fn resolve_agent_id_is_none_for_an_unpaired_token() {
    let home = tempfile::tempdir().unwrap();
    pairing::add(home.path(), "claude-code", pairing::hash("token-a")).unwrap();
    assert_eq!(
        resolve_agent_id(home.path(), Some(&pairing::hash("token-b"))),
        None
    );
}

#[test]
fn resolve_agent_id_returns_the_paired_id_for_a_matching_token() {
    let home = tempfile::tempdir().unwrap();
    pairing::add(home.path(), "claude-code", pairing::hash("token-a")).unwrap();
    assert_eq!(
        resolve_agent_id(home.path(), Some(&pairing::hash("token-a"))),
        Some("claude-code".to_owned())
    );
}

#[test]
fn a_visible_command_is_validated_before_any_session_is_looked_up() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let harness = Harness::new(&mut app).await;

        for params in [
            serde_json::json!({ "command": "pwd", "cwd": "/etc" }),
            serde_json::json!({ "command": "  " }),
            serde_json::json!({ "command": "sleep 1", "timeout_secs": 0 }),
            serde_json::json!({ "command": "id", "agent": "Gemini CLI" }),
        ] {
            let result = harness
                .call(
                    ActionKind::RemoteExecVisible,
                    params.clone(),
                    session_id("999"),
                )
                .await;
            assert_eq!(error_code(result), ErrorCode::InvalidParams, "{params}");
        }
    });
}

fn missing_host(alias: &str) -> crate::host_directory::Host {
    crate::host_directory::Host {
        missing: true,
        tags: vec!["prod".to_owned()],
        ..crate::host_directory::Host::new(alias, crate::host_directory::HostSource::SshConfig)
    }
}

/// Runs `remote.host.list` once per entry of `params_list`, in an app whose directory holds hosts
/// that are gone from the SSH configuration, so that no `ssh` is started.
async fn list_hosts_with(
    app: &mut warpui::App,
    params_list: &[serde_json::Value],
) -> Vec<Result<serde_json::Value, ControlError>> {
    let harness = Harness::new(app).await;
    app.update(crate::settings::WarpSyncSettings::register);
    let directory = app.add_singleton_model(|_| crate::host_directory::HostDirectoryModel::new());
    directory.update(app, |directory, ctx| {
        directory.replace_hosts(vec![missing_host("old-b"), missing_host("old-a")], ctx)
    });
    let mut results = Vec::new();
    for params in params_list {
        results.push(
            harness
                .call(ActionKind::RemoteHostList, params.clone(), None)
                .await,
        );
    }
    results
}

async fn list_hosts(
    app: &mut warpui::App,
    params: serde_json::Value,
) -> Result<serde_json::Value, ControlError> {
    list_hosts_with(app, &[params]).await.remove(0)
}

#[test]
fn host_list_is_unsupported_when_the_directory_is_off() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
        FeatureFlag::AgentOpsHosts.override_enabled(false),
    );
    warpui::App::test((), |mut app| async move {
        let result = list_hosts(&mut app, serde_json::json!({})).await;
        assert_eq!(error_code(result), ErrorCode::UnsupportedAction);
    });
}

#[test]
fn host_list_describes_the_directory_without_a_session() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
        FeatureFlag::AgentOpsHosts.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let data = list_hosts(&mut app, serde_json::json!({})).await.unwrap();

        assert_eq!(data["total"], 2);
        let aliases: Vec<_> = data["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|host| host["alias"].as_str().unwrap())
            .collect();
        assert_eq!(aliases, ["old-b", "old-a"]);
        assert_eq!(data["hosts"][0]["missing"], true);
        assert_eq!(data["hosts"][0]["tags"], serde_json::json!(["prod"]));
        assert_eq!(data["hosts"][0]["sessions"], serde_json::json!([]));
    });
}

#[test]
fn host_list_filters_and_limits() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
        FeatureFlag::AgentOpsHosts.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let mut results = list_hosts_with(
            &mut app,
            &[
                serde_json::json!({ "query": "old-b", "limit": 1 }),
                serde_json::json!({ "limit": 1 }),
            ],
        )
        .await
        .into_iter();

        let data = results.next().unwrap().unwrap();
        assert_eq!(data["total"], 1);
        assert_eq!(data["hosts"][0]["alias"], "old-b");

        let data = results.next().unwrap().unwrap();
        assert_eq!(data["total"], 2);
        assert_eq!(data["hosts"].as_array().unwrap().len(), 1);
    });
}

#[test]
fn host_list_rejects_bad_params() {
    let _flags = (
        FeatureFlag::WarpControlCli.override_enabled(true),
        FeatureFlag::AgentBridge.override_enabled(true),
        FeatureFlag::AgentOpsHosts.override_enabled(true),
    );
    warpui::App::test((), |mut app| async move {
        let bad = [
            serde_json::json!({ "limit": 0 }),
            serde_json::json!({ "limit": 101 }),
            serde_json::json!({ "unknown": 1 }),
        ];
        let results = list_hosts_with(&mut app, &bad).await;
        for (params, result) in bad.iter().zip(results) {
            assert_eq!(error_code(result), ErrorCode::InvalidParams, "{params}");
        }
    });
}
