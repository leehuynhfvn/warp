use super::*;

#[test]
fn request_envelope_serializes_stable_action_names() {
    let request = RequestEnvelope::new(Action::new(ActionKind::WindowFocus));
    let value = serde_json::to_value(&request).expect("request serializes");
    assert_eq!(value["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(value["action"]["kind"], "window.focus");
}

#[test]
fn strict_params_serialize_without_synthetic_discriminators() {
    let action = Action::with_params(
        ActionKind::SettingList,
        SettingListParams {
            namespace: Some("editor".to_owned()),
        },
    )
    .expect("setting.list params serialize");
    assert_eq!(action.params, serde_json::json!({ "namespace": "editor" }));
    let params = action
        .params_as::<SettingListParams>()
        .expect("setting.list params decode");
    assert_eq!(params.namespace.as_deref(), Some("editor"));

    let action = Action::with_params(
        ActionKind::TabCreate,
        TabCreateParams {
            tab_type: Some(TabType::Agent),
        },
    )
    .expect("tab.create params serialize");
    assert_eq!(action.params, serde_json::json!({ "tab_type": "agent" }));
    assert!(action.params.get("type").is_none());
    assert!(action.params.get("shell").is_none());

    let action = Action {
        kind: ActionKind::TabCreate,
        params: serde_json::json!({ "tab_type": "terminal", "shell": "zsh" }),
    };
    let error = action
        .params_as::<TabCreateParams>()
        .expect_err("shell is not an accepted tab.create parameter");
    assert_eq!(error.code, ErrorCode::InvalidParams);
}

#[test]
fn strict_params_deny_unknown_fields() {
    let action = Action {
        kind: ActionKind::InputInsert,
        params: serde_json::json!({ "text": "hello", "submit": true }),
    };
    let error = action
        .params_as::<TextParams>()
        .expect_err("unknown params are rejected");
    assert_eq!(error.code, ErrorCode::InvalidParams);

    let action = Action {
        kind: ActionKind::WindowFocus,
        params: serde_json::json!({ "unexpected": true }),
    };
    assert!(action.params_as::<EmptyParams>().is_err());
}

#[test]
fn target_selector_roundtrips_exact_session_id() {
    let target = TargetSelector {
        session: Some(SessionTarget::Id {
            id: SessionSelector("session_1".to_owned()),
        }),
        ..TargetSelector::default()
    };
    let value = serde_json::to_value(&target).expect("target serializes");
    assert_eq!(
        value["session"],
        serde_json::json!({ "type": "id", "id": "session_1" })
    );
    assert_eq!(
        serde_json::from_value::<TargetSelector>(value).expect("target decodes"),
        target
    );
}

#[test]
fn response_error_serializes_machine_code() {
    let response = ResponseEnvelope::error(
        Uuid::nil(),
        ControlError::new(ErrorCode::InsufficientPermissions, "wrong action"),
    );
    let value = serde_json::to_value(&response).expect("response serializes");
    assert_eq!(value["response"]["status"], "error");
    assert_eq!(
        value["response"]["error"]["code"],
        "insufficient_permissions"
    );
}

#[test]
fn surface_list_result_serializes_stable_availability_shape() {
    let result = SurfaceListResult {
        surfaces: vec![
            SurfaceSummary {
                name: "theme_picker".to_owned(),
                is_available: true,
                unavailable_reason: None,
            },
            SurfaceSummary {
                name: "vertical_tabs".to_owned(),
                is_available: false,
                unavailable_reason: Some("vertical tabs are disabled".to_owned()),
            },
        ],
    };
    let value = serde_json::to_value(result).expect("surface list result serializes");
    assert_eq!(
        value,
        serde_json::json!({
            "surfaces": [
                {
                    "name": "theme_picker",
                    "is_available": true
                },
                {
                    "name": "vertical_tabs",
                    "is_available": false,
                    "unavailable_reason": "vertical tabs are disabled"
                }
            ]
        })
    );
}

#[test]
fn malformed_and_removed_action_names_are_not_deserialized() {
    for action in [
        "tab.create.extra",
        "auth.status",
        "auth.login",
        "block.list",
        "block.inspect",
        "block.output",
        "history.list",
        "file.list",
        "input.get",
        "input.clear",
        "input.mode.set",
        "input.run",
        "drive.list",
        "drive.inspect",
        "drive.open",
        "drive.notebook.open",
        "drive.env_var_collection.open",
        "drive.object.share.open",
        "drive.object.create",
        "drive.object.update",
        "drive.object.delete",
        "drive.object.insert",
        "drive.object.share_to_team",
        "drive.workflow.run",
    ] {
        assert!(serde_json::from_value::<ActionKind>(serde_json::json!(action)).is_err());
    }
}

#[test]
fn catalog_has_exactly_the_retained_and_sync_actions() {
    const RETAINED_ACTIONS: usize = 84;
    const SYNC_ACTIONS: usize = 6;
    const REMOTE_ACTIONS: usize = 6;
    assert_eq!(
        ActionKind::ALL.len(),
        RETAINED_ACTIONS + SYNC_ACTIONS + REMOTE_ACTIONS
    );
}

#[test]
fn action_names_are_unique() {
    let names: std::collections::HashSet<&str> =
        ActionKind::ALL.iter().map(|kind| kind.as_str()).collect();
    assert_eq!(names.len(), ActionKind::ALL.len());
}

#[test]
fn direct_surface_actions_have_stable_names() {
    assert_eq!(ActionKind::SurfaceList.as_str(), "surface.list");
    assert_eq!(
        ActionKind::SurfaceThemePickerOpen.as_str(),
        "surface.theme_picker.open"
    );
    assert_eq!(
        ActionKind::SurfaceKeybindingsOpen.as_str(),
        "surface.keybindings.open"
    );
    assert_eq!(
        ActionKind::SurfaceCodeReviewOpen.as_str(),
        "surface.code_review.open"
    );
    assert_eq!(
        ActionKind::SurfaceProjectExplorerOpen.as_str(),
        "surface.project_explorer.open"
    );
    assert_eq!(
        ActionKind::SurfaceGlobalSearchOpen.as_str(),
        "surface.global_search.open"
    );
    assert_eq!(
        ActionKind::SurfaceConversationListOpen.as_str(),
        "surface.conversation_list.open"
    );
    assert_eq!(
        ActionKind::SurfaceVerticalTabsOpen.as_str(),
        "surface.vertical_tabs.open"
    );
    assert_eq!(
        ActionKind::SurfaceAgentManagementOpen.as_str(),
        "surface.agent_management.open"
    );
}

#[test]
fn catalog_actions_share_uniform_authorization() {
    for kind in ActionKind::ALL {
        let metadata = kind.metadata();
        assert_eq!(
            metadata.implementation_status,
            ActionImplementationStatus::Implemented,
            "{} should be implemented",
            metadata.name,
        );
    }
}

#[test]
fn implemented_catalog_contains_all_retained_actions() {
    let actions = ActionKind::implemented_metadata()
        .into_iter()
        .map(|metadata| metadata.kind)
        .collect::<Vec<_>>();
    assert_eq!(actions, ActionKind::ALL);
}

#[test]
fn sync_actions_have_stable_names() {
    let names: Vec<&str> = [
        ActionKind::SyncStatus,
        ActionKind::SyncDownload,
        ActionKind::SyncUploadPrepare,
        ActionKind::SyncConfirm,
        ActionKind::SyncCancel,
        ActionKind::SyncCompare,
    ]
    .into_iter()
    .map(ActionKind::as_str)
    .collect();

    assert_eq!(
        names,
        [
            "sync.status",
            "sync.download",
            "sync.upload.prepare",
            "sync.confirm",
            "sync.cancel",
            "sync.compare"
        ]
    );
}

#[test]
fn sync_params_reject_unknown_fields_and_malformed_ids() {
    let action = Action {
        kind: ActionKind::SyncDownload,
        params: serde_json::json!({ "path": "/m/h/etc", "recursive": true }),
    };
    assert_eq!(
        action
            .params_as::<SyncPathParams>()
            .expect_err("unknown params are rejected")
            .code,
        ErrorCode::InvalidParams
    );

    let action = Action {
        kind: ActionKind::SyncConfirm,
        params: serde_json::json!({ "pending_id": "7" }),
    };
    assert_eq!(
        action
            .params_as::<SyncPendingParams>()
            .expect_err("a pending id is a UUID")
            .code,
        ErrorCode::InvalidParams
    );

    let action = Action {
        kind: ActionKind::SyncStatus,
        params: serde_json::json!({}),
    };
    assert_eq!(
        action
            .params_as::<SyncStatusParams>()
            .expect("the path is optional"),
        SyncStatusParams::default()
    );
}

fn roundtrip(result: &SyncResult) -> serde_json::Value {
    let value = serde_json::to_value(result).expect("result serializes");
    let decoded = serde_json::from_value::<SyncResult>(value.clone()).expect("result decodes");
    assert_eq!(&decoded, result);
    value
}

#[test]
fn a_confirmation_result_flattens_its_kind_next_to_the_status() {
    let pending_id = Uuid::new_v4();
    let value = roundtrip(&SyncResult::NeedsConfirmation {
        pending_id,
        confirmation: SyncConfirmation::OverwriteLocalChanges {
            files: vec!["/etc/a".to_owned()],
        },
    });

    assert_eq!(value["status"], "needs_confirmation");
    assert_eq!(value["kind"], "overwrite_local_changes");
    assert_eq!(value["pending_id"], pending_id.to_string());
    assert_eq!(value["files"], serde_json::json!(["/etc/a"]));
}

#[test]
fn an_upload_confirmation_carries_the_summary_and_conflicts() {
    let value = roundtrip(&SyncResult::NeedsConfirmation {
        pending_id: Uuid::new_v4(),
        confirmation: SyncConfirmation::Upload {
            summary: Box::new(SyncUploadSummary {
                remote_user: "root".to_owned(),
                hostname: "prod-1".to_owned(),
                remote_path: "/etc/nginx".to_owned(),
                files: 2,
                dirs: 1,
                bytes: 3000,
                new_files: vec!["/etc/nginx/new.conf".to_owned()],
                new_file_modes: BTreeMap::new(),
                creates_under: None,
                world_writable: Vec::new(),
                runs_code: Vec::new(),
                missing_locally: Vec::new(),
                remote_conflicts: Some(SyncRemoteConflicts {
                    changed: vec!["/etc/nginx/nginx.conf".to_owned()],
                    ..SyncRemoteConflicts::default()
                }),
                ownership_may_be_incomplete: false,
                server_id_tail: None,
            }),
        },
    });

    assert_eq!(value["kind"], "upload");
    assert_eq!(value["summary"]["remote_user"], "root");
    assert!(value["summary"].get("new_file_modes").is_none());
    assert!(value["summary"].get("creates_under").is_none());
    assert!(value["summary"].get("world_writable").is_none());
    assert!(value["summary"].get("runs_code").is_none());
    assert_eq!(
        value["summary"]["remote_conflicts"]["changed"],
        serde_json::json!(["/etc/nginx/nginx.conf"])
    );
    assert!(value["summary"].get("server_id_tail").is_none());
}

#[test]
fn an_upload_summary_names_what_is_created_with_the_modes_it_gets() {
    let summary = SyncUploadSummary {
        remote_user: "root".to_owned(),
        hostname: "prod-1".to_owned(),
        remote_path: "/root/new-dir".to_owned(),
        files: 1,
        dirs: 1,
        bytes: 3,
        new_files: vec!["/root/new-dir".to_owned(), "/root/new-dir/a".to_owned()],
        new_file_modes: BTreeMap::from([
            ("/root/new-dir".to_owned(), "0755".to_owned()),
            ("/root/new-dir/a".to_owned(), "0600".to_owned()),
        ]),
        creates_under: Some("/root".to_owned()),
        world_writable: vec!["/root/new-dir/a".to_owned()],
        runs_code: vec!["/root/.ssh/authorized_keys".to_owned()],
        missing_locally: Vec::new(),
        remote_conflicts: None,
        ownership_may_be_incomplete: false,
        server_id_tail: None,
    };

    let value = roundtrip(&SyncResult::NeedsConfirmation {
        pending_id: Uuid::new_v4(),
        confirmation: SyncConfirmation::Upload {
            summary: Box::new(summary),
        },
    });

    assert_eq!(value["summary"]["creates_under"], "/root");
    assert_eq!(value["summary"]["world_writable"][0], "/root/new-dir/a");
    assert_eq!(
        value["summary"]["runs_code"][0],
        "/root/.ssh/authorized_keys"
    );
    assert_eq!(
        value["summary"]["new_file_modes"]["/root/new-dir/a"],
        "0600"
    );
}

#[test]
fn an_upload_summary_from_an_older_app_still_parses() {
    let older = serde_json::json!({
        "remote_user": "root", "hostname": "h", "remote_path": "/etc/x",
        "files": 1, "dirs": 0, "bytes": 1,
        "new_files": [], "missing_locally": [],
        "ownership_may_be_incomplete": false
    });

    let summary: SyncUploadSummary = serde_json::from_value(older).unwrap();

    assert!(summary.new_file_modes.is_empty());
    assert_eq!(summary.creates_under, None);
    assert!(summary.world_writable.is_empty() && summary.runs_code.is_empty());
}

#[test]
fn every_sync_result_shape_roundtrips() {
    let results = [
        SyncResult::Status {
            mirror_root: "/m".to_owned(),
            path: Some(SyncPathStatus {
                host_key: "prod-1".to_owned(),
                remote_path: None,
                sessions: vec![SyncSessionSummary {
                    session_id: "s1".to_owned(),
                    window_id: "w1".to_owned(),
                    tab_index: 0,
                    hostname: "prod-1".to_owned(),
                    user: "root".to_owned(),
                    is_active: true,
                }],
            }),
        },
        SyncResult::Downloaded {
            local_path: "/m/prod-1/etc".to_owned(),
            files: 1,
            dirs: 1,
            bytes: 10,
            remote_user: "root".to_owned(),
            skipped: vec![SyncSkippedEntry {
                path: "/etc/l".to_owned(),
                reason: "symbolic link".to_owned(),
            }],
            baseline_warning: None,
        },
        SyncResult::Uploaded {
            files: 1,
            dirs: 0,
            bytes: 5,
            remote_user: "root".to_owned(),
            backup_path: Some("/root/.warp-sync/backups/b.tgz".to_owned()),
            baseline_warning: None,
        },
        SyncResult::Compared {
            differences: vec![SyncDifference {
                remote_path: "/etc/a".to_owned(),
                change: SyncChange::ChangedOnServer,
                on_both_sides: true,
            }],
            identical_files: 4,
            diff_path: "/m/.warp-sync/diffs/h/etc.diff".to_owned(),
            host_dir: "/m/h".to_owned(),
            server_copy_dir: "/m/.warp-sync/compare/h".to_owned(),
            remote_user: "root".to_owned(),
        },
        SyncResult::Unchanged { identical_files: 2 },
        SyncResult::Cancelled,
    ];

    for result in &results {
        roundtrip(result);
    }
}

#[test]
fn remote_actions_have_stable_names_and_session_targets() {
    let actions = [
        (
            ActionKind::RemoteSessionList,
            "remote.session.list",
            TargetScope::Instance,
        ),
        (ActionKind::RemoteExec, "remote.exec", TargetScope::Session),
        (
            ActionKind::RemoteExecVisible,
            "remote.exec.visible",
            TargetScope::Session,
        ),
        (
            ActionKind::RemoteFileRead,
            "remote.file.read",
            TargetScope::Session,
        ),
        (
            ActionKind::RemoteFileWrite,
            "remote.file.write",
            TargetScope::Session,
        ),
        (
            ActionKind::RemoteOutputRecent,
            "remote.output.recent",
            TargetScope::Session,
        ),
    ];
    for (kind, name, scope) in actions {
        assert_eq!(kind.as_str(), name);
        assert_eq!(kind.metadata().target_scope, scope);
        assert_eq!(
            serde_json::to_value(kind).expect("kind serializes"),
            serde_json::json!(name)
        );
    }
}

#[test]
fn write_expectation_serializes_with_a_type_tag() {
    let must_not_exist =
        serde_json::to_value(WriteExpectation::MustNotExist).expect("expectation serializes");
    assert_eq!(
        must_not_exist,
        serde_json::json!({ "type": "must_not_exist" })
    );

    let sha256 = "a".repeat(64);
    let must_match = serde_json::to_value(WriteExpectation::MustMatch {
        sha256: sha256.clone(),
    })
    .expect("expectation serializes");
    assert_eq!(
        must_match,
        serde_json::json!({ "type": "must_match", "sha256": sha256 })
    );

    let parsed: WriteExpectation = serde_json::from_value(must_match).expect("round trips");
    assert_eq!(parsed, WriteExpectation::MustMatch { sha256 });
}

#[test]
fn write_expectation_rejects_unknown_types_and_fields() {
    for value in [
        serde_json::json!({ "type": "overwrite" }),
        serde_json::json!({ "type": "must_match" }),
        serde_json::json!({ "type": "must_match", "sha256": "abc", "force": true }),
        serde_json::json!({}),
    ] {
        assert!(
            serde_json::from_value::<WriteExpectation>(value.clone()).is_err(),
            "{value} should be rejected"
        );
    }
}

#[test]
fn remote_params_roundtrip_and_omit_absent_options() {
    let exec = Action::with_params(
        ActionKind::RemoteExec,
        RemoteExecParams {
            command: "nginx -t".to_owned(),
            cwd: None,
            timeout_secs: None,
            agent: None,
        },
    )
    .expect("remote.exec params serialize");
    assert_eq!(exec.params, serde_json::json!({ "command": "nginx -t" }));
    let full = RemoteExecParams {
        command: "ls".to_owned(),
        cwd: Some("/etc".to_owned()),
        timeout_secs: Some(30),
        agent: Some("claude-code".to_owned()),
    };
    let action = Action::with_params(ActionKind::RemoteExec, full.clone()).expect("serializes");
    assert_eq!(
        action.params_as::<RemoteExecParams>().expect("decodes"),
        full
    );

    let visible = Action::with_params(
        ActionKind::RemoteExecVisible,
        RemoteExecVisibleParams {
            command: "systemctl status nginx".to_owned(),
            timeout_secs: None,
            agent: None,
        },
    )
    .expect("remote.exec.visible params serialize");
    assert_eq!(
        visible.params,
        serde_json::json!({ "command": "systemctl status nginx" })
    );
    let full_visible = RemoteExecVisibleParams {
        command: "ls".to_owned(),
        timeout_secs: Some(30),
        agent: Some("claude-code".to_owned()),
    };
    let action =
        Action::with_params(ActionKind::RemoteExecVisible, full_visible.clone()).expect("serializes");
    assert_eq!(
        action
            .params_as::<RemoteExecVisibleParams>()
            .expect("decodes"),
        full_visible
    );

    let read = RemoteFileReadParams {
        path: "/etc/hosts".to_owned(),
        agent: Some("codex".to_owned()),
    };
    let action = Action::with_params(ActionKind::RemoteFileRead, read.clone()).expect("serializes");
    assert_eq!(
        action.params_as::<RemoteFileReadParams>().expect("decodes"),
        read
    );

    let write = RemoteFileWriteParams {
        path: "/etc/hosts".to_owned(),
        content_base64: "aGk=".to_owned(),
        expectation: WriteExpectation::MustNotExist,
        agent: None,
    };
    let action =
        Action::with_params(ActionKind::RemoteFileWrite, write.clone()).expect("serializes");
    assert!(action.params.get("agent").is_none());
    assert_eq!(
        action
            .params_as::<RemoteFileWriteParams>()
            .expect("decodes"),
        write
    );

    let recent = Action::with_params(
        ActionKind::RemoteOutputRecent,
        RemoteOutputRecentParams {
            count: Some(3),
            agent: None,
        },
    )
    .expect("serializes");
    assert_eq!(recent.params, serde_json::json!({ "count": 3 }));
    assert_eq!(
        Action::new(ActionKind::RemoteOutputRecent)
            .params_as::<RemoteOutputRecentParams>()
            .expect("count is optional"),
        RemoteOutputRecentParams::default()
    );
}

#[test]
fn remote_params_deny_unknown_fields_and_missing_required_ones() {
    for (kind, params) in [
        (
            ActionKind::RemoteExec,
            serde_json::json!({ "command": "ls", "shell": "zsh" }),
        ),
        (ActionKind::RemoteExec, serde_json::json!({ "cwd": "/" })),
        (
            ActionKind::RemoteFileRead,
            serde_json::json!({ "path": "/etc/hosts", "offset": 3 }),
        ),
        (
            ActionKind::RemoteFileWrite,
            serde_json::json!({ "path": "/x", "content_base64": "", "force": true,
                                "expectation": { "type": "must_not_exist" } }),
        ),
        (
            ActionKind::RemoteFileWrite,
            serde_json::json!({ "path": "/x", "content_base64": "" }),
        ),
        (
            ActionKind::RemoteOutputRecent,
            serde_json::json!({ "count": 1, "session": "x" }),
        ),
        (
            ActionKind::RemoteExecVisible,
            serde_json::json!({ "command": "pwd", "cwd": "/etc" }),
        ),
        (
            ActionKind::RemoteExecVisible,
            serde_json::json!({ "timeout_secs": 5 }),
        ),
    ] {
        let action = Action { kind, params };
        let error = match kind {
            ActionKind::RemoteExec => action.params_as::<RemoteExecParams>().err(),
            ActionKind::RemoteFileRead => action.params_as::<RemoteFileReadParams>().err(),
            ActionKind::RemoteFileWrite => action.params_as::<RemoteFileWriteParams>().err(),
            ActionKind::RemoteOutputRecent => action.params_as::<RemoteOutputRecentParams>().err(),
            ActionKind::RemoteExecVisible => action.params_as::<RemoteExecVisibleParams>().err(),
            _ => None,
        };
        let error = error.unwrap_or_else(|| panic!("{} params should be rejected", kind.as_str()));
        assert_eq!(error.code, ErrorCode::InvalidParams);
    }
}

#[test]
fn remote_error_codes_serialize_as_machine_codes() {
    for (code, name) in [
        (ErrorCode::SessionNotAttached, "session_not_attached"),
        (ErrorCode::SessionBusy, "session_busy"),
        (ErrorCode::Timeout, "timeout"),
        (ErrorCode::RemoteOperationFailed, "remote_operation_failed"),
    ] {
        assert_eq!(
            serde_json::to_value(code).expect("serializes"),
            serde_json::json!(name)
        );
        assert_eq!(code.to_string(), name);
    }
}

#[test]
fn visible_exec_result_has_no_exit_code_while_running() {
    let result = RemoteExecVisibleResult {
        session: RemoteSessionRef {
            session_id: "12".to_owned(),
            host: "prod-1".to_owned(),
            user: "root".to_owned(),
        },
        command: "sleep 30".to_owned(),
        cwd: None,
        exit_code: None,
        still_running: true,
        alt_screen: false,
        duration_ms: 5000,
        output: String::new(),
        output_rows: 0,
        truncated: false,
    };
    let value = serde_json::to_value(&result).expect("serializes");
    assert_eq!(value["exit_code"], serde_json::Value::Null);
    assert_eq!(value["still_running"], true);
    assert_eq!(value["host"], "prod-1");
    assert!(value.get("cwd").is_none());
    let parsed: RemoteExecVisibleResult = serde_json::from_value(value).expect("round trips");
    assert_eq!(parsed, result);
}
