use std::collections::{BTreeMap, HashSet};

use clap_complete::aot::Shell;
use local_control::protocol::{
    ActionKind, ControlError, ErrorCode, RemoteAccess, RemoteAttachment, RemoteExecResult,
    RemoteFileWriteResult, RemoteSessionKind, RemoteSessionRef, RemoteSessionSummary,
    RemoteStream, SyncChange, SyncConfirmation, SyncDifference,
    SyncPathStatus, SyncRemoteConflicts, SyncResult, SyncSessionSummary, SyncSkippedEntry,
    SyncUploadSummary,
};
use serde_json::json;

use super::*;

#[test]
fn parses_typed_create_and_setting_list_params() {
    let args = ControlArgs::try_parse_from([
        "warpctrl",
        "tab",
        "create",
        "--type",
        "agent",
        "--session",
        "session_1",
    ])
    .expect("tab create parses");
    let ControlCommand::Tab(TabCommand::Create(args)) = args.command else {
        panic!("expected tab create command");
    };
    assert_eq!(args.tab_type, Some(CliTabType::Agent));
    assert_eq!(args.target.session.as_deref(), Some("session_1"));

    let err = ControlArgs::try_parse_from(["warpctrl", "tab", "create", "--shell", "zsh"])
        .expect_err("shell is not an accepted tab create flag");
    assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);

    let args =
        ControlArgs::try_parse_from(["warpctrl", "setting", "list", "--namespace", "editor"])
            .expect("setting list parses");
    let ControlCommand::Setting(SettingCommand::List(args)) = args.command else {
        panic!("expected setting list command");
    };
    assert_eq!(args.namespace.as_deref(), Some("editor"));
}

#[test]
fn rejects_conflicting_instance_selectors() {
    let err = ControlArgs::try_parse_from([
        "warpctrl",
        "tab",
        "create",
        "--instance",
        "inst_123",
        "--pid",
        "123",
    ])
    .expect_err("instance and pid conflict");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn parses_instance_and_pid_selectors() {
    let args = ControlArgs::try_parse_from(["warpctrl", "tab", "create", "--instance", "inst_123"])
        .expect("instance selector parses");
    let ControlCommand::Tab(TabCommand::Create(create)) = args.command else {
        panic!("expected tab create command");
    };
    assert_eq!(create.target.instance.as_deref(), Some("inst_123"));

    let args = ControlArgs::try_parse_from(["warpctrl", "app", "ping", "--pid", "123"])
        .expect("pid selector parses");
    let ControlCommand::App(AppCommand::Ping(target)) = args.command else {
        panic!("expected app ping command");
    };
    assert_eq!(target.pid, Some(123));
}

#[test]
fn surface_list_accepts_instance_selection() {
    let args =
        ControlArgs::try_parse_from(["warpctrl", "surface", "list", "--instance", "inst_123"])
            .expect("surface list instance selector parses");
    let ControlCommand::Surface(SurfaceCommand::List(target)) = args.command else {
        panic!("expected surface list command");
    };
    assert_eq!(target.instance.as_deref(), Some("inst_123"));
}

#[test]
fn rejects_excluded_command_routes() {
    for args in [
        vec!["warpctrl", "history", "list"],
        vec!["warpctrl", "block", "list"],
        vec!["warpctrl", "block", "inspect", "block_1"],
        vec!["warpctrl", "block", "output", "block_1"],
        vec!["warpctrl", "input", "get"],
        vec!["warpctrl", "input", "clear"],
        vec!["warpctrl", "input", "mode", "set", "agent"],
        vec!["warpctrl", "input", "run", "pwd"],
        vec!["warpctrl", "file", "list"],
        vec!["warpctrl", "drive", "list"],
        vec!["warpctrl", "auth", "status"],
    ] {
        assert!(ControlArgs::try_parse_from(args).is_err());
    }
}

#[test]
fn parses_first_slice_instance_list() {
    let args = ControlArgs::try_parse_from(["warpctrl", "instance", "list"])
        .expect("instance list parses");
    assert!(matches!(
        args.command,
        ControlCommand::Instance(InstanceCommand::List)
    ));
}

#[test]
fn parses_first_slice_app_smoke_metadata_commands() {
    assert!(ControlArgs::try_parse_from(["warpctrl", "app", "ping"]).is_ok());
    assert!(ControlArgs::try_parse_from(["warpctrl", "app", "version"]).is_ok());
    assert!(ControlArgs::try_parse_from(["warpctrl", "app", "active"]).is_ok());
    assert!(ControlArgs::try_parse_from(["warpctrl", "app", "focus"]).is_ok());
}

#[test]
fn parses_catalog_metadata_commands() {
    let args =
        ControlArgs::try_parse_from(["warpctrl", "action", "inspect", "surface.settings.open"])
            .expect("action inspect parses");
    let ControlCommand::Action(ActionCatalogCommand::Inspect { action }) = args.command else {
        panic!("expected action inspect command");
    };
    assert_eq!(action, "surface.settings.open");
    assert!(ControlArgs::try_parse_from(["warpctrl", "action", "list"]).is_ok());
    assert!(ControlArgs::try_parse_from(["warpctrl", "capability", "list"]).is_ok());
    assert!(
        ControlArgs::try_parse_from(["warpctrl", "capability", "inspect", "tab.create"]).is_ok()
    );
}

#[test]
fn parses_control_mode_args_after_hidden_flag() {
    let args = ControlArgs::try_parse_control_mode_from(["warp", "--warpctrl", "tab", "create"])
        .expect("control mode flag is present")
        .expect("control mode args parse");
    assert!(matches!(
        args.command,
        ControlCommand::Tab(TabCommand::Create(_))
    ));
}

#[test]
fn ignores_args_without_control_mode_flag() {
    assert!(ControlArgs::try_parse_control_mode_from(["warp", "tab", "create"]).is_none());
}

#[test]
fn parses_completion_generation_command() {
    let args = ControlArgs::try_parse_from(["warpctrl", "completions", "bash"])
        .expect("completions parses");
    assert!(matches!(
        args.command,
        ControlCommand::Completions {
            shell: Some(Shell::Bash)
        }
    ));
}

#[test]
fn parses_exact_window_tab_pane_and_session_selectors() {
    let args = ControlArgs::try_parse_from([
        "warpctrl",
        "session",
        "inspect",
        "--window-title",
        "docs",
        "--tab-index",
        "2",
        "--pane",
        "pane_1",
        "--session",
        "session_1",
    ])
    .expect("exact target selectors parse");
    let ControlCommand::Session(SessionCommand::Inspect(target)) = args.command else {
        panic!("expected session inspect command");
    };
    assert_eq!(target.window_title.as_deref(), Some("docs"));
    assert_eq!(target.tab_index, Some(2));
    assert_eq!(target.pane.as_deref(), Some("pane_1"));
    assert_eq!(target.session.as_deref(), Some("session_1"));
}

#[test]
fn instance_list_output_serializes_empty_and_populated_lists() {
    let empty = serde_json::to_value(commands::instance_list_output(Vec::new()))
        .expect("empty list serializes");
    assert_eq!(empty, json!({ "instances": [] }));

    let record = local_control::discovery::InstanceRecord::for_current_process(
        None,
        "dev",
        "dev.warp.Warp",
        Some("v0.1.0".to_owned()),
        Vec::new(),
    );
    let instance_id = record.instance_id.0.clone();
    let populated = serde_json::to_value(commands::instance_list_output(vec![record]))
        .expect("populated list serializes");
    assert_eq!(populated["instances"][0]["instance_id"], json!(instance_id));
    assert_eq!(populated["instances"][0]["channel"], json!("dev"));
    assert_eq!(populated["instances"][0]["app_id"], json!("dev.warp.Warp"));
    assert_eq!(populated["instances"][0]["app_version"], json!("v0.1.0"));
}

#[test]
fn excluded_actions_are_not_allowlisted_catalog_entries() {
    for excluded in ["auth.api_key.set", "file.write", "block.list"] {
        assert!(
            ActionKind::ALL
                .iter()
                .all(|action| action.as_str() != excluded)
        );
    }
}

#[test]
fn generated_bash_completions_include_readonly_commands() {
    let completions =
        generate_completion_string(Shell::Bash).expect("bash completions render to UTF-8");
    assert!(completions.contains("instance"));
    assert!(completions.contains("action"));
    assert!(completions.contains("capability"));
    assert!(!completions.contains("stubs-only"));
    assert!(completions.contains("window"));
    assert!(completions.contains("input"));
    assert!(completions.contains("completions"));
    assert!(!completions.contains("block"));
}

/// Actions that no `warpctrl` command runs yet.
const REMOTE_ACTIONS_WITHOUT_CLI: &[ActionKind] = &[ActionKind::RemoteOutputRecent];

#[test]
fn every_retained_catalog_action_has_a_parseable_cli_example() {
    let mut covered = HashSet::new();
    for (kind, argv) in retained_action_examples() {
        let args = ControlArgs::try_parse_from(argv)
            .unwrap_or_else(|err| panic!("{} parses: {err}", kind.as_str()));
        assert_eq!(parsed_action_kind(&args.command), Some(kind));
        covered.insert(kind);
    }
    let expected = ActionKind::ALL
        .iter()
        .copied()
        .filter(|kind| !REMOTE_ACTIONS_WITHOUT_CLI.contains(kind))
        .collect::<HashSet<_>>();
    let missing = expected
        .difference(&covered)
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "retained catalog actions missing parser examples: {missing:?}"
    );
}

#[test]
fn generated_bash_completions_include_mutating_command_groups() {
    let completions =
        generate_completion_string(Shell::Bash).expect("bash completions render to UTF-8");
    assert!(completions.contains("surface"));
    assert!(completions.contains("command-palette"));
    assert!(completions.contains("warp-drive"));
    assert!(completions.contains("resource-center"));
    assert!(completions.contains("activate"));
    assert!(completions.contains("split"));
    assert!(!completions.contains("history"));
    assert!(!completions.contains("share-to-team"));
}

#[test]
fn structured_error_output_uses_stable_error_code() {
    let error = ControlError::new(ErrorCode::NoInstance, "no local Warp control instances");
    let value = serde_json::to_value(ErrorSummary {
        ok: false,
        error: &error,
    })
    .expect("error summary serializes");
    assert_eq!(value["ok"], json!(false));
    assert_eq!(value["error"]["code"], json!("no_instance"));
    assert_eq!(
        value["error"]["message"],
        json!("no local Warp control instances")
    );
}

#[test]
fn renders_human_readable_tab_create_output() {
    let rendered = render_human_readable_for_test(
        local_control::protocol::ActionKind::TabCreate,
        &json!({
            "tab": {
                "id": "tab_123",
                "active_index": 2,
                "count": 3
            },
            "window": {
                "id": "window_123"
            }
        }),
    );
    assert_eq!(
        rendered,
        "Created tab tab_123 in window window_123 (active index 2, tab count 3)"
    );
}

fn retained_action_examples() -> Vec<(ActionKind, Vec<&'static str>)> {
    vec![
        (
            ActionKind::InstanceList,
            vec!["warpctrl", "instance", "list"],
        ),
        (
            ActionKind::InstanceInspect,
            vec!["warpctrl", "instance", "inspect"],
        ),
        (ActionKind::AppPing, vec!["warpctrl", "app", "ping"]),
        (ActionKind::AppVersion, vec!["warpctrl", "app", "version"]),
        (ActionKind::AppActive, vec!["warpctrl", "app", "active"]),
        (ActionKind::AppFocus, vec!["warpctrl", "app", "focus"]),
        (
            ActionKind::CapabilityList,
            vec!["warpctrl", "capability", "list"],
        ),
        (
            ActionKind::CapabilityInspect,
            vec!["warpctrl", "capability", "inspect", "tab.create"],
        ),
        (ActionKind::WindowList, vec!["warpctrl", "window", "list"]),
        (
            ActionKind::WindowInspect,
            vec!["warpctrl", "window", "inspect"],
        ),
        (
            ActionKind::WindowCreate,
            vec!["warpctrl", "window", "create"],
        ),
        (ActionKind::WindowFocus, vec!["warpctrl", "window", "focus"]),
        (ActionKind::WindowClose, vec!["warpctrl", "window", "close"]),
        (ActionKind::TabList, vec!["warpctrl", "tab", "list"]),
        (ActionKind::TabInspect, vec!["warpctrl", "tab", "inspect"]),
        (ActionKind::TabCreate, vec!["warpctrl", "tab", "create"]),
        (ActionKind::TabActivate, vec!["warpctrl", "tab", "activate"]),
        (
            ActionKind::TabMove,
            vec!["warpctrl", "tab", "move", "--direction", "next"],
        ),
        (ActionKind::TabClose, vec!["warpctrl", "tab", "close"]),
        (
            ActionKind::TabRename,
            vec!["warpctrl", "tab", "rename", "docs"],
        ),
        (
            ActionKind::TabResetName,
            vec!["warpctrl", "tab", "reset-name"],
        ),
        (
            ActionKind::TabColorSet,
            vec!["warpctrl", "tab", "color", "set", "red"],
        ),
        (
            ActionKind::TabColorClear,
            vec!["warpctrl", "tab", "color", "clear"],
        ),
        (ActionKind::PaneList, vec!["warpctrl", "pane", "list"]),
        (ActionKind::PaneInspect, vec!["warpctrl", "pane", "inspect"]),
        (
            ActionKind::PaneSplit,
            vec!["warpctrl", "pane", "split", "--direction", "right"],
        ),
        (ActionKind::PaneFocus, vec!["warpctrl", "pane", "focus"]),
        (
            ActionKind::PaneNavigate,
            vec!["warpctrl", "pane", "navigate", "--direction", "next"],
        ),
        (
            ActionKind::PaneResize,
            vec![
                "warpctrl",
                "pane",
                "resize",
                "--direction",
                "right",
                "--amount",
                "4",
            ],
        ),
        (
            ActionKind::PaneMaximize,
            vec!["warpctrl", "pane", "maximize"],
        ),
        (
            ActionKind::PaneUnmaximize,
            vec!["warpctrl", "pane", "unmaximize"],
        ),
        (ActionKind::PaneClose, vec!["warpctrl", "pane", "close"]),
        (
            ActionKind::PaneRename,
            vec!["warpctrl", "pane", "rename", "server"],
        ),
        (
            ActionKind::PaneResetName,
            vec!["warpctrl", "pane", "reset-name"],
        ),
        (ActionKind::SessionList, vec!["warpctrl", "session", "list"]),
        (
            ActionKind::SessionInspect,
            vec!["warpctrl", "session", "inspect"],
        ),
        (
            ActionKind::SessionActivate,
            vec!["warpctrl", "session", "activate"],
        ),
        (
            ActionKind::SessionPrevious,
            vec!["warpctrl", "session", "previous"],
        ),
        (ActionKind::SessionNext, vec!["warpctrl", "session", "next"]),
        (
            ActionKind::SessionReopenClosed,
            vec!["warpctrl", "session", "reopen-closed"],
        ),
        (
            ActionKind::InputInsert,
            vec!["warpctrl", "input", "insert", "hello"],
        ),
        (
            ActionKind::InputReplace,
            vec!["warpctrl", "input", "replace", "hello"],
        ),
        (ActionKind::ThemeList, vec!["warpctrl", "theme", "list"]),
        (ActionKind::ThemeGet, vec!["warpctrl", "theme", "get"]),
        (
            ActionKind::ThemeSet,
            vec!["warpctrl", "theme", "set", "Dracula"],
        ),
        (
            ActionKind::ThemeSystemSet,
            vec!["warpctrl", "theme", "system-set", "true"],
        ),
        (
            ActionKind::ThemeLightSet,
            vec!["warpctrl", "theme", "light-set", "Light"],
        ),
        (
            ActionKind::ThemeDarkSet,
            vec!["warpctrl", "theme", "dark-set", "Dark"],
        ),
        (
            ActionKind::AppearanceGet,
            vec!["warpctrl", "appearance", "get"],
        ),
        (
            ActionKind::AppearanceFontSizeIncrease,
            vec!["warpctrl", "appearance", "font-size-increase"],
        ),
        (
            ActionKind::AppearanceFontSizeDecrease,
            vec!["warpctrl", "appearance", "font-size-decrease"],
        ),
        (
            ActionKind::AppearanceFontSizeReset,
            vec!["warpctrl", "appearance", "font-size-reset"],
        ),
        (
            ActionKind::AppearanceZoomIncrease,
            vec!["warpctrl", "appearance", "zoom-increase"],
        ),
        (
            ActionKind::AppearanceZoomDecrease,
            vec!["warpctrl", "appearance", "zoom-decrease"],
        ),
        (
            ActionKind::AppearanceZoomReset,
            vec!["warpctrl", "appearance", "zoom-reset"],
        ),
        (ActionKind::SettingList, vec!["warpctrl", "setting", "list"]),
        (
            ActionKind::SettingGet,
            vec!["warpctrl", "setting", "get", "font_size"],
        ),
        (
            ActionKind::SettingSet,
            vec!["warpctrl", "setting", "set", "font_size", "14"],
        ),
        (
            ActionKind::SettingToggle,
            vec!["warpctrl", "setting", "toggle", "autosuggestions"],
        ),
        (
            ActionKind::KeybindingList,
            vec!["warpctrl", "keybinding", "list"],
        ),
        (
            ActionKind::KeybindingGet,
            vec!["warpctrl", "keybinding", "get", "copy"],
        ),
        (ActionKind::ActionList, vec!["warpctrl", "action", "list"]),
        (
            ActionKind::ActionInspect,
            vec!["warpctrl", "action", "inspect", "tab.create"],
        ),
        (ActionKind::SurfaceList, vec!["warpctrl", "surface", "list"]),
        (
            ActionKind::SurfaceSettingsOpen,
            vec!["warpctrl", "surface", "settings", "open"],
        ),
        (
            ActionKind::SurfaceCommandPaletteOpen,
            vec!["warpctrl", "surface", "command-palette", "open"],
        ),
        (
            ActionKind::SurfaceCommandSearchOpen,
            vec!["warpctrl", "surface", "command-search", "open"],
        ),
        (
            ActionKind::SurfaceThemePickerOpen,
            vec!["warpctrl", "surface", "theme-picker", "open"],
        ),
        (
            ActionKind::SurfaceKeybindingsOpen,
            vec!["warpctrl", "surface", "keybindings", "open"],
        ),
        (
            ActionKind::SurfaceWarpDriveOpen,
            vec!["warpctrl", "surface", "warp-drive", "open"],
        ),
        (
            ActionKind::SurfaceWarpDriveToggle,
            vec!["warpctrl", "surface", "warp-drive", "toggle"],
        ),
        (
            ActionKind::SurfaceResourceCenterToggle,
            vec!["warpctrl", "surface", "resource-center", "toggle"],
        ),
        (
            ActionKind::SurfaceAiAssistantToggle,
            vec!["warpctrl", "surface", "ai-assistant", "toggle"],
        ),
        (
            ActionKind::SurfaceCodeReviewOpen,
            vec!["warpctrl", "surface", "code-review", "open"],
        ),
        (
            ActionKind::SurfaceCodeReviewToggle,
            vec!["warpctrl", "surface", "code-review", "toggle"],
        ),
        (
            ActionKind::SurfaceProjectExplorerOpen,
            vec!["warpctrl", "surface", "project-explorer", "open"],
        ),
        (
            ActionKind::SurfaceGlobalSearchOpen,
            vec!["warpctrl", "surface", "global-search", "open"],
        ),
        (
            ActionKind::SurfaceConversationListOpen,
            vec!["warpctrl", "surface", "conversation-list", "open"],
        ),
        (
            ActionKind::SurfaceLeftPanelToggle,
            vec!["warpctrl", "surface", "left-panel", "toggle"],
        ),
        (
            ActionKind::SurfaceRightPanelToggle,
            vec!["warpctrl", "surface", "right-panel", "toggle"],
        ),
        (
            ActionKind::SurfaceVerticalTabsOpen,
            vec!["warpctrl", "surface", "vertical-tabs", "open"],
        ),
        (
            ActionKind::SurfaceVerticalTabsToggle,
            vec!["warpctrl", "surface", "vertical-tabs", "toggle"],
        ),
        (
            ActionKind::SurfaceAgentManagementOpen,
            vec!["warpctrl", "surface", "agent-management", "open"],
        ),
        (
            ActionKind::FileOpen,
            vec!["warpctrl", "file", "open", "/tmp/example.txt"],
        ),
        (
            ActionKind::RemoteSessionList,
            vec!["warpctrl", "remote", "sessions"],
        ),
        (
            ActionKind::RemoteExec,
            vec!["warpctrl", "remote", "exec", "--session", "12", "--", "id"],
        ),
        (
            ActionKind::RemoteFileRead,
            vec!["warpctrl", "remote", "read", "--session", "12", "/etc/hosts"],
        ),
        (
            ActionKind::RemoteFileWrite,
            vec![
                "warpctrl", "remote", "write", "--session", "12", "/etc/hosts", "--from", "hosts",
                "--create",
            ],
        ),
        (ActionKind::SyncStatus, vec!["warpctrl", "sync", "status"]),
        (
            ActionKind::SyncDownload,
            vec!["warpctrl", "sync", "download", "/tmp/m/prod-1/etc"],
        ),
        (
            ActionKind::SyncUploadPrepare,
            vec!["warpctrl", "sync", "upload", "/tmp/m/prod-1/etc"],
        ),
        (
            ActionKind::SyncConfirm,
            vec![
                "warpctrl",
                "sync",
                "confirm",
                "67e55044-10b1-426f-9247-bb680e5fe0c8",
            ],
        ),
        (
            ActionKind::SyncCancel,
            vec![
                "warpctrl",
                "sync",
                "cancel",
                "67e55044-10b1-426f-9247-bb680e5fe0c8",
            ],
        ),
        (
            ActionKind::SyncCompare,
            vec!["warpctrl", "sync", "compare", "/tmp/m/prod-1/etc"],
        ),
    ]
}

fn parsed_action_kind(command: &ControlCommand) -> Option<ActionKind> {
    match command {
        ControlCommand::Instance(command) => match command {
            InstanceCommand::List => Some(ActionKind::InstanceList),
            InstanceCommand::Inspect(_) => Some(ActionKind::InstanceInspect),
        },
        ControlCommand::App(command) => match command {
            AppCommand::Ping(_) => Some(ActionKind::AppPing),
            AppCommand::Version(_) => Some(ActionKind::AppVersion),
            AppCommand::Active(_) => Some(ActionKind::AppActive),
            AppCommand::Focus(_) => Some(ActionKind::AppFocus),
        },
        ControlCommand::Capability(command) => match command {
            CapabilityCommand::List => Some(ActionKind::CapabilityList),
            CapabilityCommand::Inspect { .. } => Some(ActionKind::CapabilityInspect),
        },
        ControlCommand::Action(command) => match command {
            ActionCatalogCommand::List => Some(ActionKind::ActionList),
            ActionCatalogCommand::Inspect { .. } => Some(ActionKind::ActionInspect),
        },
        ControlCommand::Window(command) => match command {
            WindowCommand::List(_) => Some(ActionKind::WindowList),
            WindowCommand::Inspect(_) => Some(ActionKind::WindowInspect),
            WindowCommand::Create(_) => Some(ActionKind::WindowCreate),
            WindowCommand::Focus(_) => Some(ActionKind::WindowFocus),
            WindowCommand::Close(_) => Some(ActionKind::WindowClose),
        },
        ControlCommand::Tab(command) => match command {
            TabCommand::List(_) => Some(ActionKind::TabList),
            TabCommand::Inspect(_) => Some(ActionKind::TabInspect),
            TabCommand::Create(_) => Some(ActionKind::TabCreate),
            TabCommand::Activate(_) => Some(ActionKind::TabActivate),
            TabCommand::Move(_) => Some(ActionKind::TabMove),
            TabCommand::Close(_) => Some(ActionKind::TabClose),
            TabCommand::Rename(_) => Some(ActionKind::TabRename),
            TabCommand::ResetName(_) => Some(ActionKind::TabResetName),
            TabCommand::Color(command) => match command {
                TabColorCommand::Set(_) => Some(ActionKind::TabColorSet),
                TabColorCommand::Clear(_) => Some(ActionKind::TabColorClear),
            },
        },
        ControlCommand::Pane(command) => match command {
            PaneCommand::List(_) => Some(ActionKind::PaneList),
            PaneCommand::Inspect(_) => Some(ActionKind::PaneInspect),
            PaneCommand::Split(_) => Some(ActionKind::PaneSplit),
            PaneCommand::Focus(_) => Some(ActionKind::PaneFocus),
            PaneCommand::Navigate(_) => Some(ActionKind::PaneNavigate),
            PaneCommand::Resize(_) => Some(ActionKind::PaneResize),
            PaneCommand::Maximize(_) => Some(ActionKind::PaneMaximize),
            PaneCommand::Unmaximize(_) => Some(ActionKind::PaneUnmaximize),
            PaneCommand::Close(_) => Some(ActionKind::PaneClose),
            PaneCommand::Rename(_) => Some(ActionKind::PaneRename),
            PaneCommand::ResetName(_) => Some(ActionKind::PaneResetName),
        },
        ControlCommand::Session(command) => match command {
            SessionCommand::List(_) => Some(ActionKind::SessionList),
            SessionCommand::Inspect(_) => Some(ActionKind::SessionInspect),
            SessionCommand::Activate(_) => Some(ActionKind::SessionActivate),
            SessionCommand::Previous(_) => Some(ActionKind::SessionPrevious),
            SessionCommand::Next(_) => Some(ActionKind::SessionNext),
            SessionCommand::ReopenClosed(_) => Some(ActionKind::SessionReopenClosed),
        },
        ControlCommand::Input(command) => match command {
            InputCommand::Insert(_) => Some(ActionKind::InputInsert),
            InputCommand::Replace(_) => Some(ActionKind::InputReplace),
        },
        ControlCommand::Theme(command) => match command {
            ThemeCommand::List(_) => Some(ActionKind::ThemeList),
            ThemeCommand::Get(_) => Some(ActionKind::ThemeGet),
            ThemeCommand::Set(_) => Some(ActionKind::ThemeSet),
            ThemeCommand::SystemSet(_) => Some(ActionKind::ThemeSystemSet),
            ThemeCommand::LightSet(_) => Some(ActionKind::ThemeLightSet),
            ThemeCommand::DarkSet(_) => Some(ActionKind::ThemeDarkSet),
        },
        ControlCommand::Appearance(command) => match command {
            AppearanceCommand::Get(_) => Some(ActionKind::AppearanceGet),
            AppearanceCommand::FontSizeIncrease(_) => Some(ActionKind::AppearanceFontSizeIncrease),
            AppearanceCommand::FontSizeDecrease(_) => Some(ActionKind::AppearanceFontSizeDecrease),
            AppearanceCommand::FontSizeReset(_) => Some(ActionKind::AppearanceFontSizeReset),
            AppearanceCommand::ZoomIncrease(_) => Some(ActionKind::AppearanceZoomIncrease),
            AppearanceCommand::ZoomDecrease(_) => Some(ActionKind::AppearanceZoomDecrease),
            AppearanceCommand::ZoomReset(_) => Some(ActionKind::AppearanceZoomReset),
        },
        ControlCommand::Setting(command) => match command {
            SettingCommand::List(_) => Some(ActionKind::SettingList),
            SettingCommand::Get(_) => Some(ActionKind::SettingGet),
            SettingCommand::Set(_) => Some(ActionKind::SettingSet),
            SettingCommand::Toggle(_) => Some(ActionKind::SettingToggle),
        },
        ControlCommand::Keybinding(command) => match command {
            KeybindingCommand::List(_) => Some(ActionKind::KeybindingList),
            KeybindingCommand::Get(_) => Some(ActionKind::KeybindingGet),
        },
        ControlCommand::File(command) => match command {
            FileCommand::Open(_) => Some(ActionKind::FileOpen),
        },
        ControlCommand::Surface(command) => match command {
            SurfaceCommand::List(_) => Some(ActionKind::SurfaceList),
            SurfaceCommand::Settings(command) => match command {
                SurfaceSettingsCommand::Open(_) => Some(ActionKind::SurfaceSettingsOpen),
            },
            SurfaceCommand::CommandPalette(command) => match command {
                SurfaceQueryCommand::Open(_) => Some(ActionKind::SurfaceCommandPaletteOpen),
            },
            SurfaceCommand::CommandSearch(command) => match command {
                SurfaceQueryCommand::Open(_) => Some(ActionKind::SurfaceCommandSearchOpen),
            },
            SurfaceCommand::ThemePicker(command) => match command {
                SurfaceOpenCommand::Open(_) => Some(ActionKind::SurfaceThemePickerOpen),
            },
            SurfaceCommand::Keybindings(command) => match command {
                SurfaceOpenCommand::Open(_) => Some(ActionKind::SurfaceKeybindingsOpen),
            },
            SurfaceCommand::WarpDrive(command) => match command {
                SurfaceOpenToggleCommand::Open(_) => Some(ActionKind::SurfaceWarpDriveOpen),
                SurfaceOpenToggleCommand::Toggle(_) => Some(ActionKind::SurfaceWarpDriveToggle),
            },
            SurfaceCommand::ResourceCenter(command) => match command {
                SurfaceToggleCommand::Toggle(_) => Some(ActionKind::SurfaceResourceCenterToggle),
            },
            SurfaceCommand::AiAssistant(command) => match command {
                SurfaceToggleCommand::Toggle(_) => Some(ActionKind::SurfaceAiAssistantToggle),
            },
            SurfaceCommand::CodeReview(command) => match command {
                SurfaceOpenToggleCommand::Open(_) => Some(ActionKind::SurfaceCodeReviewOpen),
                SurfaceOpenToggleCommand::Toggle(_) => Some(ActionKind::SurfaceCodeReviewToggle),
            },
            SurfaceCommand::ProjectExplorer(command) => match command {
                SurfaceOpenCommand::Open(_) => Some(ActionKind::SurfaceProjectExplorerOpen),
            },
            SurfaceCommand::GlobalSearch(command) => match command {
                SurfaceOpenCommand::Open(_) => Some(ActionKind::SurfaceGlobalSearchOpen),
            },
            SurfaceCommand::ConversationList(command) => match command {
                SurfaceOpenCommand::Open(_) => Some(ActionKind::SurfaceConversationListOpen),
            },
            SurfaceCommand::LeftPanel(command) => match command {
                SurfaceToggleCommand::Toggle(_) => Some(ActionKind::SurfaceLeftPanelToggle),
            },
            SurfaceCommand::RightPanel(command) => match command {
                SurfaceToggleCommand::Toggle(_) => Some(ActionKind::SurfaceRightPanelToggle),
            },
            SurfaceCommand::VerticalTabs(command) => match command {
                SurfaceOpenToggleCommand::Open(_) => Some(ActionKind::SurfaceVerticalTabsOpen),
                SurfaceOpenToggleCommand::Toggle(_) => Some(ActionKind::SurfaceVerticalTabsToggle),
            },
            SurfaceCommand::AgentManagement(command) => match command {
                SurfaceOpenCommand::Open(_) => Some(ActionKind::SurfaceAgentManagementOpen),
            },
        },
        ControlCommand::Remote(command) => match command {
            RemoteCommand::Sessions(_) => Some(ActionKind::RemoteSessionList),
            RemoteCommand::Exec(_) => Some(ActionKind::RemoteExec),
            RemoteCommand::Read(_) => Some(ActionKind::RemoteFileRead),
            RemoteCommand::Write(_) => Some(ActionKind::RemoteFileWrite),
        },
        ControlCommand::Sync(command) => match command {
            SyncCommand::Status(_) => Some(ActionKind::SyncStatus),
            SyncCommand::Download(_) => Some(ActionKind::SyncDownload),
            SyncCommand::Upload(_) => Some(ActionKind::SyncUploadPrepare),
            SyncCommand::Confirm(_) => Some(ActionKind::SyncConfirm),
            SyncCommand::Cancel(_) => Some(ActionKind::SyncCancel),
            SyncCommand::Compare(_) => Some(ActionKind::SyncCompare),
        },
        ControlCommand::Completions { .. } => None,
    }
}

const PENDING_ID: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";

#[test]
fn sync_commands_take_a_path_and_the_usual_session_selector() {
    let args = ControlArgs::try_parse_from([
        "warpctrl",
        "sync",
        "download",
        "/tmp/m/prod-1/etc",
        "--session",
        "session_1",
        "--instance",
        "inst_123",
    ])
    .expect("sync download parses");
    let ControlCommand::Sync(SyncCommand::Download(args)) = args.command else {
        panic!("expected sync download");
    };
    assert_eq!(args.path, "/tmp/m/prod-1/etc");
    assert_eq!(args.target.session.as_deref(), Some("session_1"));
    assert_eq!(args.target.instance.as_deref(), Some("inst_123"));

    for subcommand in ["upload", "compare"] {
        assert!(
            ControlArgs::try_parse_from(["warpctrl", "sync", subcommand]).is_err(),
            "{subcommand} needs a path"
        );
    }
}

#[test]
fn sync_status_works_with_and_without_a_path() {
    let args = ControlArgs::try_parse_from(["warpctrl", "sync", "status"]).expect("status parses");
    let ControlCommand::Sync(SyncCommand::Status(args)) = args.command else {
        panic!("expected sync status");
    };
    assert!(args.path.is_none());

    let args = ControlArgs::try_parse_from(["warpctrl", "sync", "status", "/tmp/m/prod-1"])
        .expect("status with a path parses");
    let ControlCommand::Sync(SyncCommand::Status(args)) = args.command else {
        panic!("expected sync status");
    };
    assert_eq!(args.path.as_deref(), Some("/tmp/m/prod-1"));
}

#[test]
fn sync_confirm_and_cancel_take_a_pending_id_and_only_instance_selectors() {
    for subcommand in ["confirm", "cancel"] {
        let args = ControlArgs::try_parse_from([
            "warpctrl", "sync", subcommand, PENDING_ID, "--pid", "42",
        ])
        .expect("pending command parses");
        let (ControlCommand::Sync(SyncCommand::Confirm(args))
        | ControlCommand::Sync(SyncCommand::Cancel(args))) = args.command
        else {
            panic!("expected confirm or cancel");
        };
        assert_eq!(args.pending_id.to_string(), PENDING_ID);
        assert_eq!(args.pid, Some(42));

        assert!(
            ControlArgs::try_parse_from(["warpctrl", "sync", subcommand, "7"]).is_err(),
            "{subcommand} needs a UUID"
        );
        assert!(
            ControlArgs::try_parse_from([
                "warpctrl",
                "sync",
                subcommand,
                PENDING_ID,
                "--session",
                "s"
            ])
            .is_err(),
            "{subcommand} has no session selector"
        );
        assert!(
            ControlArgs::try_parse_from([
                "warpctrl",
                "sync",
                subcommand,
                PENDING_ID,
                "--instance",
                "i",
                "--pid",
                "1",
            ])
            .is_err()
        );
    }
}

#[test]
fn only_a_confirmation_request_exits_with_the_needs_confirmation_code() {
    let pending = SyncResult::NeedsConfirmation {
        pending_id: uuid::Uuid::new_v4(),
        confirmation: SyncConfirmation::OverwriteLocalChanges {
            files: vec!["/etc/a".to_owned()],
        },
    };

    assert_eq!(sync::exit_code(&pending), EXIT_NEEDS_CONFIRMATION);
    assert_eq!(EXIT_NEEDS_CONFIRMATION, 3);
    assert_eq!(sync::exit_code(&SyncResult::Cancelled), EXIT_SUCCESS);
    assert_eq!(
        sync::exit_code(&SyncResult::Unchanged { identical_files: 1 }),
        EXIT_SUCCESS
    );
}

#[test]
fn relative_paths_are_made_absolute_and_absolute_paths_are_kept() {
    let cwd = std::env::current_dir().expect("working directory");

    assert_eq!(
        sync::absolute_path("etc/nginx").expect("relative path"),
        cwd.join("etc/nginx").to_string_lossy()
    );
    assert_eq!(
        sync::absolute_path("/tmp/m/prod-1/etc").expect("absolute path"),
        "/tmp/m/prod-1/etc"
    );
}

fn upload_summary(remote_conflicts: Option<SyncRemoteConflicts>) -> SyncUploadSummary {
    SyncUploadSummary {
        remote_user: "root".to_owned(),
        hostname: "prod-1".to_owned(),
        remote_path: "/etc/nginx".to_owned(),
        files: 2,
        dirs: 1,
        bytes: 3072,
        new_files: vec!["/etc/nginx/new.conf".to_owned()],
        new_file_modes: BTreeMap::new(),
        creates_under: None,
        world_writable: Vec::new(),
        runs_code: Vec::new(),
        missing_locally: vec!["/etc/nginx/old.conf".to_owned()],
        remote_conflicts,
        ownership_may_be_incomplete: true,
        server_id_tail: Some("cdef".to_owned()),
    }
}

#[test]
fn an_upload_confirmation_shows_everything_the_user_must_weigh_and_how_to_answer() {
    let pending_id = uuid::Uuid::parse_str(PENDING_ID).expect("uuid");
    let text = sync::render_sync_result(&SyncResult::NeedsConfirmation {
        pending_id,
        confirmation: SyncConfirmation::Upload {
            summary: Box::new(upload_summary(Some(SyncRemoteConflicts {
                changed: vec!["/etc/nginx/nginx.conf".to_owned()],
                missing: vec!["/etc/nginx/gone.conf".to_owned()],
                already_exist: vec!["/etc/nginx/new.conf".to_owned()],
            }))),
        },
    });

    for expected in [
        "Upload /etc/nginx to root@prod-1 (machine id ending cdef): 2 files and 1 folder (3.0 KiB)",
        "/etc/nginx/new.conf",
        "will not be deleted on the server",
        "/etc/nginx/old.conf",
        "changed on the server since the last sync",
        "/etc/nginx/nginx.conf",
        "gone from the server",
        "already on the server",
        "not GNU tar",
        &format!("warpctrl sync confirm {PENDING_ID}"),
        &format!("warpctrl sync cancel {PENDING_ID}"),
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
}

#[test]
fn a_new_upload_says_what_it_creates_where_and_with_which_modes() {
    let summary = SyncUploadSummary {
        remote_path: "/root/test-dir-2".to_owned(),
        new_files: vec![
            "/root/test-dir-2".to_owned(),
            "/root/test-dir-2/a.conf".to_owned(),
        ],
        new_file_modes: BTreeMap::from([
            ("/root/test-dir-2".to_owned(), "0755".to_owned()),
            ("/root/test-dir-2/a.conf".to_owned(), "0600".to_owned()),
        ]),
        creates_under: Some("/root".to_owned()),
        missing_locally: Vec::new(),
        ..upload_summary(Some(SyncRemoteConflicts::default()))
    };

    let text = sync::render_sync_result(&SyncResult::NeedsConfirmation {
        pending_id: uuid::Uuid::new_v4(),
        confirmation: SyncConfirmation::Upload {
            summary: Box::new(summary),
        },
    });

    for expected in [
        "Creates on the server, inside /root (nothing there is replaced):",
        "  /root/test-dir-2 (mode 0755)",
        "  /root/test-dir-2/a.conf (mode 0600)",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(!text.contains("New files:"), "{text}");
}

#[test]
fn an_upload_warns_about_writable_entries_and_places_that_run_code() {
    let summary = SyncUploadSummary {
        world_writable: vec!["/srv/open.sh".to_owned()],
        runs_code: vec!["/etc/cron.d/job".to_owned()],
        ..upload_summary(Some(SyncRemoteConflicts::default()))
    };

    let text = sync::render_sync_result(&SyncResult::NeedsConfirmation {
        pending_id: uuid::Uuid::new_v4(),
        confirmation: SyncConfirmation::Upload {
            summary: Box::new(summary),
        },
    });

    for expected in [
        "WARNING: anyone on the server could change these new entries",
        "  /srv/open.sh",
        "WARNING: these new entries are where the server runs or trusts what it finds",
        "  /etc/cron.d/job",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    let clean = sync::render_sync_result(&SyncResult::NeedsConfirmation {
        pending_id: uuid::Uuid::new_v4(),
        confirmation: SyncConfirmation::Upload {
            summary: Box::new(upload_summary(Some(SyncRemoteConflicts::default()))),
        },
    });
    assert!(!clean.contains("where the server runs"), "{clean}");
}

#[test]
fn an_upload_to_a_host_that_could_not_be_checked_says_so() {
    let text = sync::render_sync_result(&SyncResult::NeedsConfirmation {
        pending_id: uuid::Uuid::new_v4(),
        confirmation: SyncConfirmation::Upload {
            summary: Box::new(upload_summary(None)),
        },
    });

    assert!(text.contains("could not be checked for changes"), "{text}");
}

#[test]
fn long_lists_are_cut_after_ten_paths() {
    let files: Vec<String> = (0..13).map(|i| format!("/etc/f{i}")).collect();

    let text = sync::render_sync_result(&SyncResult::NeedsConfirmation {
        pending_id: uuid::Uuid::new_v4(),
        confirmation: SyncConfirmation::OverwriteLocalChanges { files },
    });

    assert!(text.contains("13 files"), "{text}");
    assert!(
        text.contains("/etc/f9") && !text.contains("/etc/f10"),
        "{text}"
    );
    assert!(text.contains("... and 3 more"), "{text}");
}

#[test]
fn results_read_as_one_short_report_each() {
    let status = sync::render_sync_result(&SyncResult::Status {
        mirror_root: "/m".to_owned(),
        path: Some(SyncPathStatus {
            host_key: "prod-1".to_owned(),
            remote_path: Some("/etc".to_owned()),
            sessions: vec![SyncSessionSummary {
                session_id: "s1".to_owned(),
                window_id: "w1".to_owned(),
                tab_index: 1,
                hostname: "prod-1".to_owned(),
                user: "root".to_owned(),
                is_active: true,
            }],
        }),
    });
    assert_eq!(
        status,
        "Mirror folder: /m\nHost folder: prod-1\nRemote path: /etc\nSessions:\n  \
         root@prod-1: session s1 (window w1, tab 2), active"
    );

    let no_session = sync::render_sync_result(&SyncResult::Status {
        mirror_root: "/m".to_owned(),
        path: Some(SyncPathStatus {
            host_key: "prod-1".to_owned(),
            remote_path: None,
            sessions: Vec::new(),
        }),
    });
    assert!(no_session.contains("No open Warp session is connected to this host."));

    assert_eq!(
        sync::render_sync_result(&SyncResult::Downloaded {
            local_path: "/m/prod-1/etc".to_owned(),
            files: 1,
            dirs: 2,
            bytes: 512,
            remote_user: "root".to_owned(),
            skipped: vec![SyncSkippedEntry {
                path: "/etc/link".to_owned(),
                reason: "symbolic link".to_owned(),
            }],
            baseline_warning: None,
        }),
        "Downloaded 1 file and 2 folders (512 B) as root to /m/prod-1/etc\n  skipped /etc/link \
         (symbolic link)"
    );
    assert_eq!(
        sync::render_sync_result(&SyncResult::Uploaded {
            files: 1,
            dirs: 0,
            bytes: 2 * 1024 * 1024,
            remote_user: "root".to_owned(),
            backup_path: Some("/root/.warp-sync/backups/b.tgz".to_owned()),
            baseline_warning: None,
        }),
        "Uploaded 1 file and 0 folders (2.0 MiB) as root\nPrevious version saved to \
         /root/.warp-sync/backups/b.tgz"
    );
    assert_eq!(
        sync::render_sync_result(&SyncResult::Unchanged { identical_files: 1 }),
        "No differences: the mirror matches the server (1 file)"
    );
    assert_eq!(
        sync::render_sync_result(&SyncResult::Cancelled),
        "Cancelled. Nothing was changed."
    );
}

#[test]
fn a_comparison_lists_each_difference_with_what_changed() {
    let text = sync::render_sync_result(&SyncResult::Compared {
        differences: vec![
            SyncDifference {
                remote_path: "/etc/a".to_owned(),
                change: SyncChange::ChangedLocally,
                on_both_sides: true,
            },
            SyncDifference {
                remote_path: "/etc/b".to_owned(),
                change: SyncChange::NewOnServer,
                on_both_sides: false,
            },
        ],
        identical_files: 5,
        diff_path: "/m/.warp-sync/diffs/h/etc.diff".to_owned(),
        host_dir: "/m/h".to_owned(),
        server_copy_dir: "/m/.warp-sync/compare/h".to_owned(),
        remote_user: "root".to_owned(),
    });

    assert_eq!(
        text,
        "2 differences between the server (as root) and the mirror; 5 files identical\n  \
         changed locally: /etc/a\n  new on the server: /etc/b\nDiff saved at \
         /m/.warp-sync/diffs/h/etc.diff"
    );
}

#[test]
fn remote_exec_keeps_the_command_words_and_their_dashes() {
    let args = ControlArgs::try_parse_from([
        "warpctrl", "remote", "exec", "--session", "12", "--cwd", "/etc", "--timeout", "30", "--",
        "ls", "-la", "--color=never",
    ])
    .expect("remote exec parses");
    let ControlCommand::Remote(RemoteCommand::Exec(args)) = args.command else {
        panic!("expected remote exec");
    };
    assert_eq!(args.command, ["ls", "-la", "--color=never"]);
    assert_eq!(args.cwd.as_deref(), Some("/etc"));
    assert_eq!(args.timeout_secs, 30);
    assert_eq!(args.target.session.as_deref(), Some("12"));
}

#[test]
fn remote_exec_needs_a_command_and_defaults_to_two_minutes() {
    assert!(ControlArgs::try_parse_from(["warpctrl", "remote", "exec", "--session", "1"]).is_err());
    let args = ControlArgs::try_parse_from(["warpctrl", "remote", "exec", "--", "true"])
        .expect("remote exec parses");
    let ControlCommand::Remote(RemoteCommand::Exec(args)) = args.command else {
        panic!("expected remote exec");
    };
    assert_eq!(args.timeout_secs, 120);
}

#[test]
fn remote_write_needs_exactly_one_expectation() {
    let base = ["warpctrl", "remote", "write", "/etc/hosts", "--from", "hosts"];
    assert!(ControlArgs::try_parse_from(base).is_err(), "no expectation");

    let both = [&base[..], &["--create", "--expected-sha256", "abc"]].concat();
    assert!(ControlArgs::try_parse_from(both).is_err(), "both expectations");

    let sha = "a".repeat(64);
    let overwrite = [&base[..], &["--expected-sha256", sha.as_str()]].concat();
    let args = ControlArgs::try_parse_from(overwrite).expect("overwrite parses");
    let ControlCommand::Remote(RemoteCommand::Write(args)) = args.command else {
        panic!("expected remote write");
    };
    assert_eq!(args.expected_sha256.as_deref(), Some(sha.as_str()));
    assert!(!args.create);

    let create = [&base[..], &["--create"]].concat();
    let args = ControlArgs::try_parse_from(create).expect("create parses");
    let ControlCommand::Remote(RemoteCommand::Write(args)) = args.command else {
        panic!("expected remote write");
    };
    assert!(args.create && args.expected_sha256.is_none());
}

fn exec_result(exit_code: i32) -> RemoteExecResult {
    let stream = |text: &str| RemoteStream {
        text: text.to_owned(),
        total_bytes: text.len() as u64,
        truncated: false,
    };
    RemoteExecResult {
        session: RemoteSessionRef {
            session_id: "12".to_owned(),
            host: "prod-1".to_owned(),
            user: "root".to_owned(),
        },
        cwd: None,
        exit_code,
        timed_out: false,
        duration_ms: 5,
        stdout: stream("out"),
        stderr: stream(""),
    }
}

#[test]
fn the_exec_exit_code_is_the_remote_one_kept_in_range() {
    use remote::exec_exit_code;
    assert_eq!(exec_exit_code(&exec_result(0)), 0);
    assert_eq!(exec_exit_code(&exec_result(3)), 3);
    assert_eq!(exec_exit_code(&exec_result(124)), 124);
    assert_eq!(exec_exit_code(&exec_result(255)), 255);
    assert_eq!(exec_exit_code(&exec_result(-1)), 0);
    assert_eq!(exec_exit_code(&exec_result(1000)), 255);
}

#[test]
fn the_session_list_marks_the_active_session_and_shows_what_may_be_used() {
    use remote::render_sessions;
    let session = |id: &str, is_active: bool, attached: Option<RemoteAttachment>| {
        RemoteSessionSummary {
            session_id: id.to_owned(),
            window_index: 0,
            tab_index: 0,
            pane_index: 0,
            is_active,
            session_type: RemoteSessionKind::Remote,
            host: "prod-1".to_owned(),
            user: "root".to_owned(),
            shell: "bash".to_owned(),
            cwd: Some("/root".to_owned()),
            attached,
        }
    };
    let attached = RemoteAttachment {
        access: RemoteAccess::ReadOnly,
        idle_secs: 30,
        expires_in_secs: 29 * 60 + 30,
        exec_count: 2,
    };
    let text = render_sessions(&[session("12", true, Some(attached)), session("13", false, None)]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "* 12  root@prod-1  bash  /root  [read-only, 2 commands, expires in 29m]"
    );
    assert_eq!(lines[1], "  13  root@prod-1  bash  /root  [not attached]");
    assert_eq!(render_sessions(&[]), "No sessions.");
}

#[test]
fn a_write_result_names_the_backup_only_when_there_is_one() {
    use remote::render_write;
    let mut result = RemoteFileWriteResult {
        session: RemoteSessionRef {
            session_id: "12".to_owned(),
            host: "prod-1".to_owned(),
            user: "root".to_owned(),
        },
        path: "/etc/app.conf".to_owned(),
        bytes: 42,
        sha256: "abc".to_owned(),
        backup_path: Some("/root/.warp-agent/backups/app.conf.1".to_owned()),
        created: false,
    };
    let text = render_write(&result);
    assert!(text.starts_with("Wrote /etc/app.conf (42 bytes, sha256 abc)"), "{text}");
    assert!(text.contains("saved in /root/.warp-agent/backups/app.conf.1"), "{text}");

    result.created = true;
    result.backup_path = None;
    let text = render_write(&result);
    assert!(text.starts_with("Created /etc/app.conf"), "{text}");
    assert!(!text.contains("saved in"), "{text}");
}

#[test]
fn server_output_cannot_carry_terminal_escape_sequences_to_the_operator() {
    use remote::terminal_safe;
    assert_eq!(
        terminal_safe("ok\u{1b}[31m red\u{1b}]0;title\u{7}\ttab\r\nline\u{0}"),
        "ok[31m red]0;title\ttab\r\nline"
    );
    assert_eq!(terminal_safe("ünïcode ✓"), "ünïcode ✓");
}
