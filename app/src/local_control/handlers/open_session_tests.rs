use std::cell::Cell;
use std::fs;

use ::local_control::protocol::RemoteSessionOpenParams;
use warp_core::features::FeatureFlag;

use super::*;
use crate::agent_bridge::model::OpenReady;
use crate::agent_bridge::pairing;
use crate::host_directory::{HostSource, Resolved};
use crate::terminal::model::session::SessionId;

fn params(host: &str, purpose: &str) -> RemoteSessionOpenParams {
    RemoteSessionOpenParams {
        host: host.to_owned(),
        access: RemoteAccess::ReadOnly,
        purpose: purpose.to_owned(),
        root: false,
        wait_secs: None,
        agent: None,
    }
}

fn invalid(result: Result<ValidOpen, ControlError>) -> String {
    let error = result.expect_err("the request is invalid");
    assert_eq!(error.code, ErrorCode::InvalidParams);
    error.message
}

// --- Checking the request's own fields ----------------------------------------

#[test]
fn a_plain_request_is_read_only_without_root_and_waits_the_default() {
    let valid = validate_open(params("lab-1", "check the disk")).unwrap();
    assert_eq!(
        valid,
        ValidOpen {
            alias: "lab-1".to_owned(),
            access: Access::ReadOnly,
            purpose: "check the disk".to_owned(),
            root: false,
            wait: Duration::from_secs(OPEN_WAIT_DEFAULT_SECS.into()),
            agent: None,
        }
    );
}

#[test]
fn the_wishes_of_the_agent_are_carried_over() {
    let valid = validate_open(RemoteSessionOpenParams {
        access: RemoteAccess::Full,
        root: true,
        wait_secs: Some(15),
        agent: Some("claude-code".to_owned()),
        ..params("lab-1", "  restart nginx  ")
    })
    .unwrap();
    assert_eq!(valid.access, Access::Full);
    assert!(valid.root);
    assert_eq!(valid.wait, Duration::from_secs(15));
    assert_eq!(valid.agent.as_deref(), Some("claude-code"));
    assert_eq!(valid.purpose, "restart nginx");
}

#[test]
fn an_alias_that_could_change_the_ssh_command_is_refused() {
    for alias in [
        "-oProxyCommand=x",
        "lab-1; reboot",
        "lab 1",
        "lab-1\nid",
        "$(id)",
        "",
        &"a".repeat(65),
        "lab-é",
    ] {
        let message = invalid(validate_open(params(alias, "x")));
        assert!(message.starts_with("host \""), "{alias}: {message}");
    }
}

#[test]
fn a_purpose_has_to_be_a_short_single_line_of_text() {
    for purpose in ["", "   ", "line one\nline two", "bell\u{7}", "esc\u{1b}[2J"] {
        assert!(
            invalid(validate_open(params("lab-1", purpose))).contains("purpose"),
            "{purpose:?}"
        );
    }
    let too_long = "x".repeat(MAX_OPEN_PURPOSE_BYTES + 1);
    assert!(invalid(validate_open(params("lab-1", &too_long))).contains("longer than"));
    assert!(validate_open(params("lab-1", &"x".repeat(MAX_OPEN_PURPOSE_BYTES))).is_ok());
}

#[test]
fn the_wait_has_to_be_within_its_range() {
    for wait in [0, OPEN_WAIT_MAX_SECS + 1] {
        let message = invalid(validate_open(RemoteSessionOpenParams {
            wait_secs: Some(wait),
            ..params("lab-1", "x")
        }));
        assert!(message.contains("wait_secs"));
    }
    for wait in [1, OPEN_WAIT_MAX_SECS] {
        assert!(
            validate_open(RemoteSessionOpenParams {
                wait_secs: Some(wait),
                ..params("lab-1", "x")
            })
            .is_ok()
        );
    }
}

#[test]
fn an_agent_name_that_is_not_a_label_is_refused() {
    let message = invalid(validate_open(RemoteSessionOpenParams {
        agent: Some("bad name".to_owned()),
        ..params("lab-1", "x")
    }));
    assert!(message.contains("agent"));
}

// --- Feature gates ---------------------------------------------------------------

#[test]
fn opening_needs_the_bridge_the_policy_the_directory_and_its_own_flag() {
    let _bridge_and_policy = (
        FeatureFlag::AgentBridge.override_enabled(true),
        FeatureFlag::AgentOpsPolicy.override_enabled(true),
    );
    for (hosts, open_session, missing) in [
        (false, true, "the server directory"),
        (true, false, "agents opening sessions"),
    ] {
        let _flags = (
            FeatureFlag::AgentOpsHosts.override_enabled(hosts),
            FeatureFlag::AgentOpsOpenSession.override_enabled(open_session),
        );
        let error =
            ensure_open_enabled(ActionKind::RemoteSessionOpen).expect_err("a flag is missing");
        assert_eq!(error.code, ErrorCode::UnsupportedAction);
        assert!(error.message.contains(missing), "{}", error.message);
    }
}

#[test]
fn opening_is_allowed_when_every_flag_is_on() {
    let _flags = (
        FeatureFlag::AgentBridge.override_enabled(true),
        FeatureFlag::AgentOpsPolicy.override_enabled(true),
        FeatureFlag::AgentOpsHosts.override_enabled(true),
        FeatureFlag::AgentOpsOpenSession.override_enabled(true),
    );
    assert!(ensure_open_enabled(ActionKind::RemoteSessionOpen).is_ok());
    assert!(ensure_open_enabled(ActionKind::RemoteSessionClose).is_ok());
}

#[test]
fn without_the_bridge_nothing_opens_whatever_else_is_on() {
    let _flags = (
        FeatureFlag::AgentBridge.override_enabled(false),
        FeatureFlag::AgentOpsPolicy.override_enabled(true),
        FeatureFlag::AgentOpsHosts.override_enabled(true),
        FeatureFlag::AgentOpsOpenSession.override_enabled(true),
    );
    assert_eq!(
        ensure_open_enabled(ActionKind::RemoteSessionOpen)
            .expect_err("the bridge is off")
            .code,
        ErrorCode::UnsupportedAction
    );
}

// --- The background step -----------------------------------------------------------

struct FakeSsh {
    calls: Cell<usize>,
}

impl SshResolver for FakeSsh {
    fn resolve(&self, _config: Option<&Path>, alias: &str) -> Result<Resolved, String> {
        self.calls.set(self.calls.get() + 1);
        Ok(Resolved {
            hostname: format!("{alias}.example"),
            user: "ops".to_owned(),
            port: 22,
            proxy_jump: None,
            identity_files: Vec::new(),
        })
    }
}

fn fake_ssh() -> FakeSsh {
    FakeSsh {
        calls: Cell::new(0),
    }
}

fn lab_host() -> Host {
    Host {
        tags: vec!["lab".to_owned()],
        ..Host::new("lab-1", HostSource::SshConfig)
    }
}

fn staging(access: Access, root: bool) -> StagingRequest {
    StagingRequest {
        alias: "lab-1".to_owned(),
        tags: vec!["lab".to_owned()],
        access,
        root,
    }
}

fn home_with_policy(policy: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".warp/agent-ops");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("policy.toml");
    fs::write(&path, policy).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    home
}

fn paired(home: &Path) -> String {
    pairing::add(home, "claude-code", pairing::hash("token-a")).unwrap();
    pairing::hash("token-a")
}

const ALLOW_LAB: &str = "[defaults]\nmode = \"approve\"\n\n[[open.hosts]]\nmatch = \"lab-*\"\n\
                         mode = \"allow\"\nmax_access = \"full\"\n";

#[test]
fn an_agent_that_is_not_paired_is_refused_before_anything_else_is_read_or_run() {
    let home = home_with_policy(ALLOW_LAB);
    let ssh = fake_ssh();

    for token in [None, Some("not-a-paired-hash")] {
        let staged = stage(
            Some(home.path()),
            &ssh,
            &lab_host(),
            &staging(Access::ReadOnly, false),
            token,
        );
        assert_eq!(staged.agent_id, None);
        assert!(
            matches!(&staged.decision, Decision::Deny(reason) if reason.contains("paired")),
            "{:?}",
            staged.decision
        );
    }
    assert_eq!(
        ssh.calls.get(),
        0,
        "ssh -G must not run for an unpaired agent"
    );
}

#[test]
fn a_paired_agent_gets_what_the_policy_says() {
    let home = home_with_policy(ALLOW_LAB);
    let token = paired(home.path());
    let ssh = fake_ssh();

    let staged = stage(
        Some(home.path()),
        &ssh,
        &lab_host(),
        &staging(Access::Full, false),
        Some(&token),
    );

    assert_eq!(staged.agent_id.as_deref(), Some("claude-code"));
    assert_eq!(staged.decision, Decision::Allow);
    assert_eq!(
        ssh.calls.get(),
        0,
        "no need to resolve the server when nobody is asked"
    );
}

#[test]
fn a_server_the_policy_does_not_name_is_asked_about_and_resolved_for_the_dialog() {
    let home = home_with_policy("[defaults]\nmode = \"approve\"\n");
    let token = paired(home.path());
    let ssh = fake_ssh();

    let staged = stage(
        Some(home.path()),
        &ssh,
        &lab_host(),
        &staging(Access::ReadOnly, false),
        Some(&token),
    );

    assert_eq!(staged.decision, Decision::Ask);
    assert_eq!(staged.connection.as_deref(), Some("ops@lab-1.example:22"));
    assert_eq!(ssh.calls.get(), 1);
}

#[test]
fn a_missing_policy_file_asks_too() {
    let home = tempfile::tempdir().unwrap();
    let token = paired(home.path());

    let staged = stage(
        Some(home.path()),
        &fake_ssh(),
        &lab_host(),
        &staging(Access::ReadOnly, false),
        Some(&token),
    );

    assert_eq!(staged.decision, Decision::Ask);
    assert_eq!(staged.limits, OpenLimits::default());
}

#[test]
fn the_limits_come_from_the_policy() {
    let home = home_with_policy(
        "[defaults]\nmode = \"approve\"\n\n[open]\nmax_sessions_per_host = 1\n\
         max_sessions_per_agent = 3\n",
    );
    let token = paired(home.path());

    let staged = stage(
        Some(home.path()),
        &fake_ssh(),
        &lab_host(),
        &staging(Access::ReadOnly, false),
        Some(&token),
    );

    assert_eq!(
        staged.limits,
        OpenLimits {
            per_host: 1,
            per_agent: 3
        }
    );
}

#[test]
fn a_policy_that_cannot_be_read_refuses_and_says_how_to_fix_it() {
    for policy in ["this is = not [valid", "[defaults]\nmode = \"sometimes\"\n"] {
        let home = home_with_policy(policy);
        let token = paired(home.path());

        let staged = stage(
            Some(home.path()),
            &fake_ssh(),
            &lab_host(),
            &staging(Access::ReadOnly, false),
            Some(&token),
        );

        assert!(
            matches!(&staged.decision, Decision::Deny(reason)
                if reason.contains("policy.toml") && reason.contains("Ask the user to fix")),
            "{:?}",
            staged.decision
        );
    }
}

#[cfg(unix)]
#[test]
fn a_policy_that_others_can_write_refuses() {
    use std::os::unix::fs::PermissionsExt as _;

    let home = home_with_policy(ALLOW_LAB);
    fs::set_permissions(
        home.path().join(".warp/agent-ops/policy.toml"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    let token = paired(home.path());

    let staged = stage(
        Some(home.path()),
        &fake_ssh(),
        &lab_host(),
        &staging(Access::ReadOnly, false),
        Some(&token),
    );

    assert!(matches!(staged.decision, Decision::Deny(_)));
}

#[test]
fn without_a_home_directory_nothing_is_allowed() {
    let staged = stage(
        None,
        &fake_ssh(),
        &lab_host(),
        &staging(Access::ReadOnly, false),
        Some("token"),
    );
    assert_eq!(staged.agent_id, None);
    assert!(matches!(staged.decision, Decision::Deny(_)));
}

#[test]
fn root_counts_when_the_login_is_root_or_sudo_will_be_typed() {
    let valid = validate_open(params("lab-1", "x")).unwrap();
    let with_login = |root_login| Host {
        root_login,
        ..lab_host()
    };
    assert!(
        staging_request(
            &valid,
            &with_login(RootLogin::Root),
            &ElevationPlan::AlreadyRoot
        )
        .root
    );
    assert!(
        staging_request(
            &valid,
            &with_login(RootLogin::SudoNopasswd),
            &ElevationPlan::Run
        )
        .root
    );
    assert!(
        !staging_request(
            &valid,
            &with_login(RootLogin::SudoNopasswd),
            &ElevationPlan::NotRequested
        )
        .root
    );
    assert!(
        !staging_request(
            &valid,
            &with_login(RootLogin::SudoPassword),
            &ElevationPlan::Skipped("no".to_owned())
        )
        .root
    );
}

// --- What the agent is told ---------------------------------------------------------

fn ready(elevated: bool, root_note: Option<&str>) -> OpenReady {
    OpenReady {
        session: SessionId::from(2u64),
        user: if elevated { "root" } else { "ops" }.to_owned(),
        host: "lab-1.internal".to_owned(),
        elevated,
        root_note: root_note.map(str::to_owned),
    }
}

fn valid(access: RemoteAccess) -> ValidOpen {
    validate_open(RemoteSessionOpenParams {
        access,
        ..params("lab-1", "x")
    })
    .unwrap()
}

#[test]
fn a_ready_session_says_who_it_is_signed_in_as() {
    let result = open_result(
        &valid(RemoteAccess::Full),
        &ElevationPlan::NotRequested,
        "12",
        Some(ready(false, None)),
    );
    assert_eq!(result.status, RemoteOpenStatus::Ready);
    assert_eq!(result.session_id, "12");
    assert_eq!(result.host_alias, "lab-1");
    assert_eq!(result.user.as_deref(), Some("ops"));
    assert_eq!(result.host.as_deref(), Some("lab-1.internal"));
    assert_eq!(result.access, RemoteAccess::Full);
    assert_eq!(result.elevation, RemoteOpenElevation::NotRequested);
    assert_eq!(result.note, None);
}

#[test]
fn a_session_that_is_not_ready_can_still_be_addressed_and_says_what_to_do() {
    let result = open_result(
        &valid(RemoteAccess::ReadOnly),
        &ElevationPlan::NotRequested,
        "12",
        None,
    );
    assert_eq!(result.status, RemoteOpenStatus::Connecting);
    assert_eq!(result.session_id, "12");
    assert_eq!(result.user, None);
    let note = result.note.expect("a note explains it");
    assert!(note.contains("list_sessions") && note.contains("10 minutes"));
    assert!(note.contains("Do not open another"));
}

#[test]
fn root_that_was_reached_is_reported() {
    let result = open_result(
        &valid(RemoteAccess::Full),
        &ElevationPlan::Run,
        "12",
        Some(ready(true, None)),
    );
    assert_eq!(result.elevation, RemoteOpenElevation::Elevated);
    assert_eq!(result.user.as_deref(), Some("root"));
    assert_eq!(result.note, None);
}

#[test]
fn root_that_was_planned_but_not_reached_says_why() {
    let result = open_result(
        &valid(RemoteAccess::Full),
        &ElevationPlan::Run,
        "12",
        Some(ready(
            false,
            Some("'sudo -i' was not run because the tab was closed."),
        )),
    );
    assert_eq!(result.elevation, RemoteOpenElevation::Skipped);
    assert!(result.note.unwrap().contains("the tab was closed"));
}

#[test]
fn root_that_is_still_being_reached_is_pending() {
    let result = open_result(&valid(RemoteAccess::Full), &ElevationPlan::Run, "12", None);
    assert_eq!(result.elevation, RemoteOpenElevation::Pending);
}

#[test]
fn root_that_was_never_possible_is_skipped_with_the_reason_from_the_start() {
    let plan = ElevationPlan::Skipped("this server needs a sudo password".to_owned());
    for outcome in [Some(ready(false, None)), None] {
        let result = open_result(&valid(RemoteAccess::Full), &plan, "12", outcome);
        assert_eq!(result.elevation, RemoteOpenElevation::Skipped);
        assert!(result.note.unwrap().contains("sudo password"));
    }
}

#[test]
fn a_login_that_is_root_already_is_reported_as_such() {
    let result = open_result(
        &valid(RemoteAccess::Full),
        &ElevationPlan::AlreadyRoot,
        "12",
        Some(ready(false, None)),
    );
    assert_eq!(result.elevation, RemoteOpenElevation::AlreadyRoot);
}

// --- End to end: request, tab, Warpify, close ----------------------------------------------

mod end_to_end {
    use std::path::{Path, PathBuf};

    use ::local_control::protocol::{
        Action, AgentToken, RemoteSessionCloseResult, RequestEnvelope, SessionSelector,
        SessionTarget, TargetSelector,
    };
    use warpui::{App, ModelHandle, SingletonEntity as _, ViewHandle};

    use super::*;
    use crate::agent_bridge::approval::ApprovalDecision;
    use crate::agent_bridge::model::AgentBridgeModel;
    use crate::host_directory::HostDirectoryModel;
    use crate::local_control::handlers::close_session;
    use crate::terminal::model::session::{BootstrapSessionType, SessionInfo, Sessions};
    use crate::terminal::model::terminal_model::SubshellInitializationInfo;
    use crate::terminal::ssh::util::InteractiveSshCommand;
    use crate::workspace::view::tests::{initialize_app, mock_workspace};

    type Answer = Result<Value, ControlError>;

    /// `$HOME` for the duration of `body`, holding a policy and the tokens of paired agents.
    fn with_home(policy: &str, agents: &[&str], body: impl FnOnce(PathBuf, Vec<AgentToken>)) {
        let home = home_with_policy(policy);
        let tokens: Vec<AgentToken> = agents.iter().map(|_| AgentToken::generate()).collect();
        for (name, token) in agents.iter().zip(&tokens) {
            pairing::add(home.path(), name, pairing::hash(token.secret())).unwrap();
        }
        // The server list is filled from these when the workspace opens: `lab-1` is in the SSH
        // configuration, `lab-9` only in what Warp remembered, so it is gone from it.
        fs::create_dir_all(home.path().join(".ssh")).unwrap();
        fs::write(
            home.path().join(".ssh/config"),
            "Host lab-1\n    HostName lab-1.example\n",
        )
        .unwrap();
        fs::write(
            home.path().join(".warp/agent-ops/hosts.toml"),
            "version = 1\n\n[[hosts]]\nalias = \"lab-1\"\nsource = \"ssh_config\"\n\
             root_login = \"sudo_nopasswd\"\n\n[[hosts]]\nalias = \"lab-9\"\n\
             source = \"ssh_config\"\n",
        )
        .unwrap();
        let previous_home = std::env::var_os("HOME");
        // SAFETY: the tests that change `$HOME` are `#[serial]`, so nothing reads it meanwhile.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        body(home.path().to_owned(), tokens);
        unsafe {
            match &previous_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn flags() -> impl Sized {
        (
            FeatureFlag::AgentBridge.override_enabled(true),
            FeatureFlag::AgentOpsPolicy.override_enabled(true),
            FeatureFlag::AgentOpsHosts.override_enabled(true),
            FeatureFlag::AgentOpsOpenSession.override_enabled(true),
            FeatureFlag::GroupedTabs.override_enabled(true),
        )
    }

    struct World {
        workspace: ViewHandle<Workspace>,
        bridge: ModelHandle<LocalControlBridge>,
    }

    /// An app with a window and the server list `with_home` describes, once it has been read.
    async fn world(app: &mut App) -> World {
        initialize_app(app);
        app.add_singleton_model(|_| AgentBridgeModel::default());
        app.add_singleton_model(|_| HostDirectoryModel::new());
        let workspace = mock_workspace(app);
        let bridge = app.add_singleton_model(LocalControlBridge::new);
        until("the server list", || {
            HostDirectoryModel::handle(app).read(app, |directory, _| {
                let state = |alias: &str| {
                    directory
                        .hosts()
                        .iter()
                        .find(|host| host.alias == alias)
                        .map(|host| host.missing)
                };
                state("lab-1") == Some(false) && state("lab-9") == Some(true)
            })
        })
        .await;
        World { workspace, bridge }
    }

    fn open_request(token: Option<&AgentToken>, params: serde_json::Value) -> RequestEnvelope {
        RequestEnvelope {
            agent_token: token.cloned(),
            ..RequestEnvelope::new(Action {
                kind: ActionKind::RemoteSessionOpen,
                params,
            })
        }
    }

    fn open_params(alias: &str) -> serde_json::Value {
        serde_json::json!({ "host": alias, "purpose": "check the disk", "wait_secs": 30 })
    }

    fn start_open(
        app: &mut App,
        world: &World,
        token: Option<&AgentToken>,
        params: serde_json::Value,
    ) -> Result<RemoteReceiver, ControlError> {
        let request = open_request(token, params);
        let hash = token.map(|token| pairing::hash(token.secret()));
        world.bridge.update(app, |_, ctx| open(&request, hash, ctx))
    }

    async fn until(what: &str, mut condition: impl FnMut() -> bool) {
        for _ in 0..1000 {
            if condition() {
                return;
            }
            Timer::after(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// The pane group of the tab the user is looking at.
    fn active_tab(app: &App, world: &World) -> warpui::EntityId {
        world.workspace.read(app, |workspace, _| {
            workspace
                .get_pane_group_view(workspace.active_tab_index())
                .expect("there is an active tab")
                .id()
        })
    }

    fn tab_count(app: &App, world: &World) -> usize {
        world
            .workspace
            .read(app, |workspace, _| workspace.tab_count())
    }

    /// The pane id and the `Sessions` of the tab titled for `alias`.
    fn agent_tab(app: &App, world: &World, alias: &str) -> Option<(String, ModelHandle<Sessions>)> {
        world.workspace.read(app, |workspace, ctx| {
            (0..workspace.tab_count()).find_map(|index| {
                let pane_group = workspace.get_pane_group_view(index)?;
                let title = pane_group.as_ref(ctx).display_title(ctx);
                if title != format!("Agent · {alias}") {
                    return None;
                }
                pane_group.read(ctx, |pane_group, ctx| {
                    let pane_id = pane_group.focused_pane_id(ctx);
                    let view = pane_group.terminal_view_from_pane_id(pane_id, ctx)?;
                    let sessions = view.read(ctx, |view, _| view.sessions_model().clone());
                    Some((pane_id.to_string(), sessions))
                })
            })
        })
    }

    fn warpify(app: &mut App, sessions: &ModelHandle<Sessions>, id: u64, ssh_host: &str) {
        let info = SessionInfo {
            subshell_info: Some(SubshellInitializationInfo {
                spawning_command: format!("ssh {ssh_host}"),
                was_triggered_by_rc_file_snippet: false,
                env_var_collection_name: None,
                ssh_connection_info: Some(InteractiveSshCommand {
                    host: Some(ssh_host.to_owned()),
                    port: None,
                }),
            }),
            user: "ops".to_owned(),
            hostname: "lab-1.internal".to_owned(),
            ..SessionInfo::new_for_test()
                .with_id(id)
                .with_session_type(BootstrapSessionType::WarpifiedRemote)
        };
        sessions.update(app, |sessions, ctx| {
            sessions.initialize_bootstrapped_session(
                info,
                format!("ssh {ssh_host}"),
                vec![],
                None,
                ctx,
            );
        });
    }

    /// The shell that `sudo -i` starts in the session that `warpify` made.
    fn warpify_root(app: &mut App, sessions: &ModelHandle<Sessions>, id: u64) {
        let info = SessionInfo {
            subshell_info: Some(SubshellInitializationInfo {
                spawning_command: "sudo -i".to_owned(),
                was_triggered_by_rc_file_snippet: false,
                env_var_collection_name: None,
                ssh_connection_info: None,
            }),
            user: "root".to_owned(),
            hostname: "lab-1.internal".to_owned(),
            ..SessionInfo::new_for_test()
                .with_id(id)
                .with_session_type(BootstrapSessionType::WarpifiedRemote)
        };
        sessions.update(app, |sessions, ctx| {
            sessions.initialize_bootstrapped_session(info, "sudo -i".to_owned(), vec![], None, ctx);
        });
    }

    /// Opens `alias` and Warpifies its tab, returning what the agent was told.
    async fn open_ready(
        app: &mut App,
        world: &World,
        token: &AgentToken,
        alias: &str,
        session_id: u64,
    ) -> (String, serde_json::Value) {
        let before = tab_count(app, world);
        let receiver = start_open(app, world, Some(token), open_params(alias)).unwrap();
        until("the agent's tab", || {
            agent_tab(app, world, alias).is_some() && tab_count(app, world) > before
        })
        .await;
        let (pane, sessions) = agent_tab(app, world, alias).unwrap();
        warpify(app, &sessions, session_id, alias);
        let data = receiver.await.expect("answered").expect("opened");
        (pane, data)
    }

    fn audit_lines(home: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(home.join(".warp/agent-bridge/audit.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn close_request(token: Option<&AgentToken>, pane: &str) -> RequestEnvelope {
        RequestEnvelope {
            agent_token: token.cloned(),
            target: TargetSelector {
                session: Some(SessionTarget::Id {
                    id: SessionSelector(pane.to_owned()),
                }),
                ..TargetSelector::default()
            },
            ..RequestEnvelope::new(Action {
                kind: ActionKind::RemoteSessionClose,
                params: serde_json::json!({}),
            })
        }
    }

    async fn close_as(
        app: &mut App,
        world: &World,
        token: Option<&AgentToken>,
        pane: &str,
    ) -> Answer {
        let request = close_request(token, pane);
        let hash = token.map(|token| pairing::hash(token.secret()));
        let receiver = world
            .bridge
            .update(app, |_, ctx| close_session::close(&request, hash, ctx))?;
        receiver.await.expect("answered")
    }

    #[test]
    #[serial_test::serial]
    fn a_paired_agent_on_an_allowed_server_gets_a_tab_that_is_attached_once_it_warpifies() {
        let _flags = flags();
        with_home(ALLOW_LAB, &["claude-code"], |home, tokens| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                let user_tab = active_tab(&app, &world);

                let (pane, data) = open_ready(&mut app, &world, &tokens[0], "lab-1", 41).await;

                assert_eq!(data["status"], "ready");
                assert_eq!(data["session_id"], pane.as_str());
                assert_eq!(data["host_alias"], "lab-1");
                assert_eq!(data["user"], "ops");
                assert_eq!(data["host"], "lab-1.internal");
                assert_eq!(data["access"], "read_only");
                assert_eq!(data["elevation"], "not_requested");
                assert_eq!(active_tab(&app, &world), user_tab, "focus is not taken");
                AgentBridgeModel::handle(&app).read(&app, |model, _| {
                    let status = model
                        .status(SessionId::from(41u64))
                        .expect("the Warpified session is attached");
                    assert_eq!(status.access, Access::ReadOnly);
                    assert!(model.opened_by(&pane, "claude-code").is_some());
                });

                let lines = audit_lines(&home);
                let results: Vec<_> = lines.iter().map(|line| line["result"].as_str()).collect();
                assert_eq!(results, [Some("started"), Some("ok")]);
                for line in &lines {
                    assert_eq!(line["action"], "remote.session.open");
                    assert_eq!(line["agent_id"], "claude-code");
                    assert_eq!(line["purpose"], "check the disk");
                    assert_eq!(line["policy_decision"], "allow");
                }
            });
        });
    }

    #[test]
    #[serial_test::serial]
    fn an_agent_that_is_not_paired_opens_nothing() {
        let _flags = flags();
        with_home(ALLOW_LAB, &[], |home, _| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                let before = tab_count(&app, &world);

                let receiver = start_open(&mut app, &world, None, open_params("lab-1")).unwrap();
                let error = receiver.await.expect("answered").expect_err("refused");

                assert_eq!(error.code, ErrorCode::PolicyDenied);
                assert!(error.message.contains("paired"), "{}", error.message);
                assert_eq!(tab_count(&app, &world), before);
                let lines = audit_lines(&home);
                assert_eq!(lines.len(), 1);
                assert_eq!(lines[0]["error_code"], "policy_denied");
            });
        });
    }

    #[test]
    #[serial_test::serial]
    fn a_server_no_rule_names_is_asked_about_and_a_denial_opens_nothing() {
        let _flags = flags();
        with_home(
            "[defaults]\nmode = \"approve\"\n",
            &["claude-code"],
            |home, tokens| {
                App::test((), |mut app| async move {
                    let world = world(&mut app).await;
                    let before = tab_count(&app, &world);

                    let receiver =
                        start_open(&mut app, &world, Some(&tokens[0]), open_params("lab-1"))
                            .unwrap();
                    until("the request to wait for the user", || {
                        AgentBridgeModel::handle(&app)
                            .read(&app, |model, _| model.has_pending_open())
                    })
                    .await;
                    assert_eq!(
                        tab_count(&app, &world),
                        before,
                        "nothing opens before approval"
                    );
                    let request_id = AgentBridgeModel::handle(&app).read(&app, |model, _| {
                        model.pending_open_request_ids().first().copied().unwrap()
                    });
                    AgentBridgeModel::handle(&app).update(&mut app, |model, ctx| {
                        model.decide_approval(request_id, ApprovalDecision::Deny, ctx)
                    });

                    let error = receiver.await.expect("answered").expect_err("refused");
                    assert_eq!(error.code, ErrorCode::PolicyDenied);
                    assert!(error.message.contains("the user denied it"));
                    assert_eq!(tab_count(&app, &world), before);
                    let decisions: Vec<_> = audit_lines(&home)
                        .iter()
                        .map(|line| line["result"].as_str().map(str::to_owned))
                        .collect();
                    assert_eq!(
                        decisions,
                        [
                            Some("approval_requested".to_owned()),
                            Some("error".to_owned())
                        ]
                    );
                });
            },
        );
    }

    #[test]
    #[serial_test::serial]
    fn an_approved_request_opens_the_tab_and_the_session_is_attached() {
        let _flags = flags();
        with_home(
            "[defaults]\nmode = \"approve\"\n",
            &["claude-code"],
            |home, tokens| {
                App::test((), |mut app| async move {
                    let world = world(&mut app).await;
                    let receiver =
                        start_open(&mut app, &world, Some(&tokens[0]), open_params("lab-1"))
                            .unwrap();
                    until("the request to wait for the user", || {
                        AgentBridgeModel::handle(&app)
                            .read(&app, |model, _| model.has_pending_open())
                    })
                    .await;
                    let request_id = AgentBridgeModel::handle(&app).read(&app, |model, _| {
                        model.pending_open_request_ids().first().copied().unwrap()
                    });
                    AgentBridgeModel::handle(&app).update(&mut app, |model, ctx| {
                        model.decide_approval(request_id, ApprovalDecision::Approve, ctx)
                    });
                    until("the agent's tab", || {
                        agent_tab(&app, &world, "lab-1").is_some()
                    })
                    .await;
                    let (_, sessions) = agent_tab(&app, &world, "lab-1").unwrap();
                    warpify(&mut app, &sessions, 51, "lab-1");

                    let data = receiver.await.expect("answered").expect("opened");
                    assert_eq!(data["status"], "ready");
                    let decisions: Vec<_> = audit_lines(&home)
                        .iter()
                        .filter_map(|line| line["policy_decision"].as_str().map(str::to_owned))
                        .collect();
                    assert!(decisions.contains(&"approved".to_owned()), "{decisions:?}");
                });
            },
        );
    }

    #[test]
    #[serial_test::serial]
    fn a_session_that_does_not_warpify_in_time_is_reported_as_connecting_and_attaches_late() {
        let _flags = flags();
        with_home(ALLOW_LAB, &["claude-code"], |_, tokens| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                let params = serde_json::json!({
                    "host": "lab-1", "purpose": "x", "wait_secs": 1
                });

                let receiver = start_open(&mut app, &world, Some(&tokens[0]), params).unwrap();
                let data = receiver.await.expect("answered").expect("not an error");

                assert_eq!(data["status"], "connecting");
                let pane = data["session_id"].as_str().unwrap().to_owned();
                assert!(data["note"].as_str().unwrap().contains("list_sessions"));
                AgentBridgeModel::handle(&app).read(&app, |model, _| {
                    assert!(model.opened_by(&pane, "claude-code").is_some());
                });

                let (_, sessions) = agent_tab(&app, &world, "lab-1").unwrap();
                warpify(&mut app, &sessions, 61, "lab-1");
                AgentBridgeModel::handle(&app).read(&app, |model, _| {
                    assert!(
                        model.status(SessionId::from(61u64)).is_some(),
                        "the session attaches by itself when it becomes ready"
                    );
                });
            });
        });
    }

    #[test]
    #[serial_test::serial]
    fn the_limit_per_server_stops_a_second_session() {
        let _flags = flags();
        let policy = format!("{ALLOW_LAB}\n[open]\nmax_sessions_per_host = 1\n");
        with_home(&policy, &["claude-code"], |_, tokens| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                open_ready(&mut app, &world, &tokens[0], "lab-1", 71).await;
                let before = tab_count(&app, &world);

                let receiver =
                    start_open(&mut app, &world, Some(&tokens[0]), open_params("lab-1")).unwrap();
                let error = receiver.await.expect("answered").expect_err("refused");

                assert_eq!(error.code, ErrorCode::PolicyDenied);
                assert!(
                    error
                        .message
                        .contains("already have 1 sessions open on lab-1")
                );
                assert_eq!(tab_count(&app, &world), before);
            });
        });
    }

    #[test]
    #[serial_test::serial]
    fn a_server_that_is_not_in_the_list_or_is_gone_from_it_is_refused() {
        let _flags = flags();
        with_home(ALLOW_LAB, &["claude-code"], |_, tokens| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                for alias in ["lab-9", "lab-404"] {
                    let error = start_open(&mut app, &world, Some(&tokens[0]), open_params(alias))
                        .expect_err("refused at once");
                    assert_eq!(error.code, ErrorCode::InvalidParams, "{alias}");
                }
            });
        });
    }

    #[test]
    #[serial_test::serial]
    fn an_agent_closes_its_own_session_and_only_its_own() {
        let _flags = flags();
        with_home(ALLOW_LAB, &["claude-code", "codex"], |home, tokens| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                let (pane, _) = open_ready(&mut app, &world, &tokens[0], "lab-1", 81).await;
                let before = tab_count(&app, &world);

                let stranger = close_as(&mut app, &world, Some(&tokens[1]), &pane).await;
                assert_eq!(stranger.unwrap_err().code, ErrorCode::PolicyDenied);
                let unpaired = close_as(&mut app, &world, None, &pane).await;
                assert_eq!(unpaired.unwrap_err().code, ErrorCode::PolicyDenied);
                assert_eq!(tab_count(&app, &world), before, "nothing was closed");

                let closed = close_as(&mut app, &world, Some(&tokens[0]), &pane)
                    .await
                    .expect("the owner may close it");
                let closed: RemoteSessionCloseResult = serde_json::from_value(closed).unwrap();
                assert_eq!(closed.session_id, pane);
                assert!(closed.closed);
                assert_eq!(tab_count(&app, &world), before - 1);
                AgentBridgeModel::handle(&app).read(&app, |model, _| {
                    assert!(model.opened_by(&pane, "claude-code").is_none());
                    assert!(model.status(SessionId::from(81u64)).is_none());
                });
                let close_lines: Vec<_> = audit_lines(&home)
                    .into_iter()
                    .filter(|line| line["action"] == "remote.session.close")
                    .collect();
                assert_eq!(close_lines.len(), 2, "started and ok: {close_lines:?}");
                assert!(
                    close_lines
                        .iter()
                        .all(|line| line["agent_id"] == "claude-code")
                );
            });
        });
    }

    #[test]
    #[serial_test::serial]
    fn a_session_the_user_opened_by_hand_cannot_be_closed_by_an_agent() {
        let _flags = flags();
        with_home(ALLOW_LAB, &["claude-code"], |_, tokens| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                let (user_pane, before) = world.workspace.read(&app, |workspace, ctx| {
                    let pane_group = workspace.get_pane_group_view(0).unwrap();
                    let pane = pane_group.read(ctx, |pane_group, ctx| {
                        pane_group.focused_pane_id(ctx).to_string()
                    });
                    (pane, workspace.tab_count())
                });

                let refused = close_as(&mut app, &world, Some(&tokens[0]), &user_pane).await;

                let error = refused.expect_err("not the agent's to close");
                assert_eq!(error.code, ErrorCode::PolicyDenied);
                assert!(error.message.contains("not opened by you"));
                assert_eq!(tab_count(&app, &world), before);
            });
        });
    }

    #[test]
    #[serial_test::serial]
    fn closing_needs_the_id_of_a_session() {
        let _flags = flags();
        with_home(ALLOW_LAB, &["claude-code"], |_, tokens| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                let request = RequestEnvelope {
                    agent_token: Some(tokens[0].clone()),
                    ..RequestEnvelope::new(Action {
                        kind: ActionKind::RemoteSessionClose,
                        params: serde_json::json!({}),
                    })
                };
                let error = world
                    .bridge
                    .update(&mut app, |_, ctx| close_session::close(&request, None, ctx))
                    .expect_err("no session named");
                assert_eq!(error.code, ErrorCode::MissingTarget);
            });
        });
    }

    const ALLOW_LAB_AND_ROOT: &str = "[defaults]\nmode = \"approve\"\n\n[[open.hosts]]\n\
        match = \"lab-*\"\nmode = \"allow\"\nmax_access = \"full\"\nallow_root = true\n";

    fn allow_sudo_as_a_subshell(app: &mut App) {
        use crate::terminal::warpify::settings::WarpifySettings;
        WarpifySettings::handle(app).update(app, |settings, ctx| {
            settings
                .added_subshell_commands
                .set_value(vec!["sudo -i".to_owned()], ctx)
                .expect("the setting applies");
        });
    }

    #[test]
    #[serial_test::serial]
    fn root_is_not_reached_when_warpify_would_not_take_sudo_and_the_agent_is_told() {
        let _flags = flags();
        with_home(ALLOW_LAB_AND_ROOT, &["claude-code"], |_, tokens| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                let receiver = start_open(
                    &mut app,
                    &world,
                    Some(&tokens[0]),
                    serde_json::json!({
                        "host": "lab-1", "purpose": "x", "root": true, "access": "full",
                        "wait_secs": 30
                    }),
                )
                .unwrap();
                until("the agent's tab", || {
                    agent_tab(&app, &world, "lab-1").is_some()
                })
                .await;
                let (_, sessions) = agent_tab(&app, &world, "lab-1").unwrap();
                warpify(&mut app, &sessions, 91, "lab-1");

                let data = receiver.await.expect("answered").expect("opened");

                assert_eq!(data["status"], "ready");
                assert_eq!(data["user"], "ops");
                assert_eq!(data["elevation"], "skipped");
                assert!(data["note"].as_str().unwrap().contains("Warpify"), "{data}");
            });
        });
    }

    #[test]
    #[serial_test::serial]
    fn the_root_shell_takes_over_the_attachment_from_the_login_users_session() {
        let _flags = flags();
        with_home(ALLOW_LAB_AND_ROOT, &["claude-code"], |_, tokens| {
            App::test((), |mut app| async move {
                let world = world(&mut app).await;
                allow_sudo_as_a_subshell(&mut app);
                let receiver = start_open(
                    &mut app,
                    &world,
                    Some(&tokens[0]),
                    serde_json::json!({
                        "host": "lab-1", "purpose": "x", "root": true, "access": "full",
                        "wait_secs": 30
                    }),
                )
                .unwrap();
                until("the agent's tab", || {
                    agent_tab(&app, &world, "lab-1").is_some()
                })
                .await;
                let (_, sessions) = agent_tab(&app, &world, "lab-1").unwrap();
                warpify(&mut app, &sessions, 92, "lab-1");

                // What was typed into the tab, and what the shell answered, is up to the real
                // terminal; what matters here is what Warp does with the two sessions.
                AgentBridgeModel::handle(&app).read(&app, |model, _| {
                    assert!(model.status(SessionId::from(92u64)).is_some());
                });
                warpify_root(&mut app, &sessions, 93);

                let data = receiver.await.expect("answered").expect("opened");
                AgentBridgeModel::handle(&app).read(&app, |model, _| {
                    if data["elevation"] == "elevated" {
                        assert_eq!(data["user"], "root");
                        assert!(model.status(SessionId::from(93u64)).is_some());
                        assert!(
                            model.status(SessionId::from(92u64)).is_none(),
                            "the login user's session is detached"
                        );
                    } else {
                        assert_eq!(data["elevation"], "skipped", "{data}");
                        assert!(model.status(SessionId::from(92u64)).is_some());
                    }
                });
            });
        });
    }
}
