use std::time::{Duration, SystemTime};

use futures::executor::block_on;
use warpui::{App, ModelHandle, WindowId};

use super::*;
use crate::agent_bridge::approval::{
    AgentLabel, ApprovalDecision, ApprovalRequest, ApprovalSubject,
};

fn setup(app: &mut App) -> ModelHandle<AgentBridgeModel> {
    app.add_singleton_model(|_| AgentBridgeModel::default())
}

fn session(id: u64) -> SessionId {
    SessionId::from(id)
}

fn window(id: usize) -> WindowId {
    WindowId::from_usize(id)
}

fn request(session: SessionId, window_id: WindowId) -> ApprovalRequest {
    ApprovalRequest {
        request_id: Uuid::new_v4(),
        session: Some(session),
        session_label: "root@lab-1".to_owned(),
        agent: AgentLabel {
            claimed: Some("claude-code".to_owned()),
            agent_id: None,
        },
        subject: ApprovalSubject::Command {
            command: "uptime".to_owned(),
            cwd: None,
            visible: false,
        },
        deadline: SystemTime::now() + Duration::from_secs(300),
        window_id,
    }
}

#[test]
fn push_approval_then_decide_approval_delivers_the_decision_and_clears_the_queue() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);
        let req = request(session(1), window(1));
        let id = req.request_id;

        let receiver = model
            .update(&mut app, |model, ctx| model.push_approval(req, ctx))
            .expect("queue has room");
        model.read(&app, |model, _| {
            assert!(model.approval(id).is_some(), "the request is queued");
        });

        let decided = model.update(&mut app, |model, ctx| {
            model.decide_approval(id, ApprovalDecision::Approve, ctx)
        });
        assert!(decided);
        assert_eq!(block_on(receiver), Ok(ApprovalDecision::Approve));
        model.read(&app, |model, _| {
            assert!(
                model.approval(id).is_none(),
                "a decided request leaves the queue"
            );
        });
    });
}

#[test]
fn deciding_an_unknown_request_returns_false() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);
        let decided = model.update(&mut app, |model, ctx| {
            model.decide_approval(Uuid::new_v4(), ApprovalDecision::Deny, ctx)
        });
        assert!(!decided);
    });
}

#[test]
fn remove_approval_drops_the_request_without_a_decision() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);
        let req = request(session(1), window(1));
        let id = req.request_id;

        let receiver = model
            .update(&mut app, |model, ctx| model.push_approval(req, ctx))
            .expect("queue has room");
        let removed = model.update(&mut app, |model, ctx| model.remove_approval(id, ctx));
        assert!(removed);
        assert!(
            block_on(receiver).is_err(),
            "removing without deciding drops the sender"
        );
    });
}

#[test]
fn pending_and_oldest_approvals_are_tracked_per_session() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);
        let first = request(session(1), window(1));
        let first_id = first.request_id;
        drop(
            model
                .update(&mut app, |model, ctx| model.push_approval(first, ctx))
                .expect("queue has room"),
        );
        drop(
            model
                .update(&mut app, |model, ctx| {
                    model.push_approval(request(session(1), window(1)), ctx)
                })
                .expect("queue has room"),
        );
        drop(
            model
                .update(&mut app, |model, ctx| {
                    model.push_approval(request(session(2), window(2)), ctx)
                })
                .expect("queue has room"),
        );

        model.read(&app, |model, _| {
            assert_eq!(model.pending_approvals_for_session(session(1)), 2);
            assert_eq!(model.pending_approvals_for_session(session(2)), 1);
            assert_eq!(model.pending_approvals_for_session(session(3)), 0);
            assert_eq!(
                model
                    .oldest_approval_for_session(session(1))
                    .map(|r| r.request_id),
                Some(first_id)
            );
            assert_eq!(
                model
                    .oldest_approval_in_window(window(1))
                    .map(|r| r.request_id),
                Some(first_id)
            );
            assert!(model.oldest_approval_in_window(window(3)).is_none());
        });
    });
}

#[test]
fn deny_all_approvals_revokes_and_clears_every_pending_request() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);
        let a_rx = model
            .update(&mut app, |model, ctx| {
                model.push_approval(request(session(1), window(1)), ctx)
            })
            .expect("queue has room");
        let b_rx = model
            .update(&mut app, |model, ctx| {
                model.push_approval(request(session(2), window(2)), ctx)
            })
            .expect("queue has room");

        let count = model.update(&mut app, |model, ctx| model.deny_all_approvals(ctx));
        assert_eq!(count, 2);
        assert_eq!(block_on(a_rx), Ok(ApprovalDecision::Revoked));
        assert_eq!(block_on(b_rx), Ok(ApprovalDecision::Revoked));
        model.read(&app, |model, _| {
            assert_eq!(model.pending_approvals_for_session(session(1)), 0);
            assert_eq!(model.pending_approvals_for_session(session(2)), 0);
        });
    });
}

#[test]
fn detach_revokes_that_sessions_pending_approvals_but_not_other_sessions() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);
        model.update(&mut app, |model, ctx| {
            model.attach(session(1), Access::Full, ctx)
        });
        let revoked_rx = model
            .update(&mut app, |model, ctx| {
                model.push_approval(request(session(1), window(1)), ctx)
            })
            .expect("queue has room");
        let untouched_rx = model
            .update(&mut app, |model, ctx| {
                model.push_approval(request(session(2), window(1)), ctx)
            })
            .expect("queue has room");

        let was_attached = model.update(&mut app, |model, ctx| model.detach(session(1), ctx));
        assert!(was_attached);
        assert_eq!(block_on(revoked_rx), Ok(ApprovalDecision::Revoked));
        model.read(&app, |model, _| {
            assert_eq!(model.pending_approvals_for_session(session(2)), 1);
        });
        drop(untouched_rx);
    });
}

#[test]
fn detach_all_revokes_every_pending_approval() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);
        let rx = model
            .update(&mut app, |model, ctx| {
                model.push_approval(request(session(1), window(1)), ctx)
            })
            .expect("queue has room");

        model.update(&mut app, |model, ctx| model.detach_all(ctx));
        assert_eq!(block_on(rx), Ok(ApprovalDecision::Revoked));
    });
}

#[test]
fn a_command_allowed_in_session_is_recognized_through_the_model_wrapper() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);
        model.update(&mut app, |model, ctx| {
            model.attach(session(1), Access::Full, ctx)
        });

        assert!(!model.read(&app, |model, _| {
            model.is_command_allowed_in_session(session(1), "uptime")
        }));
        model.update(&mut app, |model, _| {
            model.allow_command_in_session(session(1), "uptime")
        });
        assert!(model.read(&app, |model, _| {
            model.is_command_allowed_in_session(session(1), "uptime")
        }));
    });
}

#[test]
fn trusting_an_attached_session_is_recognized_through_the_model_wrapper() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);
        model.update(&mut app, |model, ctx| {
            model.attach(session(1), Access::Full, ctx)
        });

        assert!(!model.read(&app, |model, _| model.is_session_trusted(session(1))));
        assert!(model.update(&mut app, |model, ctx| model.trust_session(session(1), ctx)));
        assert!(model.read(&app, |model, _| model.is_session_trusted(session(1))));
    });
}

#[test]
fn trusting_a_session_that_is_not_attached_does_nothing() {
    App::test((), |mut app| async move {
        let model = setup(&mut app);

        assert!(!model.update(&mut app, |model, ctx| model.trust_session(session(1), ctx)));
        assert!(!model.read(&app, |model, _| model.is_session_trusted(session(1))));
    });
}

// --- Sessions an agent opened ---------------------------------------------------

mod opened {
    use warpui::{App, SingletonEntity};

    use super::*;
    use crate::agent_bridge::opened::{OpenState, Opened};
    use crate::agent_bridge::policy::OpenLimits;
    use crate::terminal::History;
    use crate::terminal::model::session::{BootstrapSessionType, SessionInfo};
    use crate::terminal::model::terminal_model::SubshellInitializationInfo;
    use crate::terminal::ssh::util::InteractiveSshCommand;

    const LIMITS: OpenLimits = OpenLimits {
        per_host: 2,
        per_agent: 4,
    };

    fn opened(elevate: bool) -> Opened {
        Opened {
            agent_id: "claude-code".to_owned(),
            alias: "lab-1".to_owned(),
            access: Access::Full,
            elevate,
            opened_at: Instant::now(),
            state: OpenState::Connecting,
        }
    }

    fn ssh_session(id: u64, ssh_host: &str) -> SessionInfo {
        SessionInfo {
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
        }
    }

    fn local_session(id: u64) -> SessionInfo {
        SessionInfo::new_for_test().with_id(id)
    }

    fn setup_open(
        app: &mut App,
        elevate: bool,
    ) -> (
        ModelHandle<AgentBridgeModel>,
        ModelHandle<Sessions>,
        oneshot::Receiver<OpenReady>,
    ) {
        crate::test_util::settings::initialize_settings_for_tests(app);
        app.add_singleton_model(|_| crate::auth::AuthStateProvider::new_for_test());
        app.add_singleton_model(
            crate::server::telemetry::context_provider::AppTelemetryContextProvider::new_context_provider,
        );
        app.add_singleton_model(|_| History::new(vec![]));
        let model = setup(app);
        let sessions = app.add_model(|_| Sessions::new_for_test());
        let sessions_for_model = sessions.clone();
        let receiver = model.update(app, |model, ctx| {
            model.track_open(
                "7".to_owned(),
                opened(elevate),
                &sessions_for_model,
                None,
                ctx,
            )
        });
        (model, sessions, receiver)
    }

    fn bootstrap(app: &mut App, sessions: &ModelHandle<Sessions>, info: SessionInfo) {
        let spawning_command = info
            .subshell_info
            .as_ref()
            .map(|subshell| subshell.spawning_command.clone())
            .unwrap_or_default();
        sessions.update(app, |sessions, ctx| {
            sessions.initialize_bootstrapped_session(info, spawning_command, vec![], None, ctx);
        });
    }

    #[test]
    fn the_session_that_warpifies_in_the_tab_is_attached_and_the_request_answered() {
        App::test((), |mut app| async move {
            let (model, sessions, receiver) = setup_open(&mut app, false);

            bootstrap(&mut app, &sessions, local_session(1));
            model.read(&app, |model, _| assert!(model.status(session(1)).is_none()));

            bootstrap(&mut app, &sessions, ssh_session(2, "lab-1"));
            assert_eq!(
                block_on(receiver),
                Ok(OpenReady {
                    session: session(2),
                    user: "ops".to_owned(),
                    host: "lab-1.internal".to_owned(),
                    elevated: false,
                    root_note: None,
                })
            );
            model.read(&app, |model, _| {
                let status = model.status(session(2)).expect("the session is attached");
                assert_eq!(status.access, Access::Full);
                assert!(model.status(session(1)).is_none());
            });
        });
    }

    #[test]
    fn a_session_to_another_server_does_not_answer_the_request() {
        App::test((), |mut app| async move {
            let (model, sessions, mut receiver) = setup_open(&mut app, false);

            bootstrap(&mut app, &sessions, ssh_session(2, "lab-2"));
            assert!(matches!(receiver.try_recv(), Ok(None)));
            model.read(&app, |model, _| assert!(model.status(session(2)).is_none()));
        });
    }

    #[test]
    fn a_root_shell_that_cannot_be_started_leaves_the_login_users_session_attached() {
        App::test((), |mut app| async move {
            let (model, sessions, receiver) = setup_open(&mut app, true);

            bootstrap(&mut app, &sessions, ssh_session(2, "lab-1"));
            let ready = block_on(receiver).expect("the request is answered");
            assert_eq!(ready.session, session(2));
            assert!(!ready.elevated);
            let note = ready.root_note.expect("the agent is told why");
            assert!(note.contains("sudo -i") && note.contains("login user"));
            model.read(&app, |model, _| {
                assert!(model.status(session(2)).is_some());
                assert_eq!(
                    model
                        .opened_by("7", "claude-code")
                        .map(|opened| opened.state),
                    Some(OpenState::Ready {
                        session: session(2)
                    })
                );
            });
        });
    }

    #[test]
    fn only_the_agent_that_opened_a_session_owns_it() {
        App::test((), |mut app| async move {
            let (model, _sessions, _receiver) = setup_open(&mut app, false);
            model.read(&app, |model, _| {
                assert!(model.opened_by("7", "claude-code").is_some());
                assert!(model.opened_by("7", "codex").is_none());
                assert!(model.opened_by("8", "claude-code").is_none());
            });
        });
    }

    #[test]
    fn forgetting_a_session_takes_away_its_attachment_and_frees_its_slot() {
        App::test((), |mut app| async move {
            let (model, sessions, _receiver) = setup_open(&mut app, false);
            bootstrap(&mut app, &sessions, ssh_session(2, "lab-1"));

            let forgotten = model.update(&mut app, |model, ctx| model.forget_opened("7", ctx));
            assert_eq!(
                forgotten.map(|opened| opened.alias),
                Some("lab-1".to_owned())
            );
            model.read(&app, |model, _| assert!(model.status(session(2)).is_none()));

            let live = HashSet::from(["7".to_owned()]);
            model.update(&mut app, |model, _| {
                assert!(model.opened_by("7", "claude-code").is_none());
                assert_eq!(
                    model.check_open_limits("claude-code", "lab-1", LIMITS, &live),
                    Ok(())
                );
            });
        });
    }

    #[test]
    fn a_session_whose_tab_was_closed_stops_counting_against_the_limits() {
        App::test((), |mut app| async move {
            let (model, _sessions, _receiver) = setup_open(&mut app, false);
            let one_per_host = OpenLimits {
                per_host: 1,
                per_agent: 4,
            };

            model.update(&mut app, |model, _| {
                let live = HashSet::from(["7".to_owned()]);
                assert!(
                    model
                        .check_open_limits("claude-code", "lab-1", one_per_host, &live)
                        .is_err()
                );
                let none_live = HashSet::new();
                assert_eq!(
                    model.check_open_limits("claude-code", "lab-1", one_per_host, &none_live),
                    Ok(())
                );
            });
        });
    }

    #[test]
    fn detaching_the_session_keeps_the_tab_registered_so_it_can_still_be_closed() {
        App::test((), |mut app| async move {
            let (model, sessions, _receiver) = setup_open(&mut app, false);
            bootstrap(&mut app, &sessions, ssh_session(2, "lab-1"));

            model.update(&mut app, |model, ctx| {
                assert!(model.detach(session(2), ctx));
            });
            model.read(&app, |model, _| {
                assert!(model.status(session(2)).is_none());
                assert!(model.opened_by("7", "claude-code").is_some());
            });
            let _ = AgentBridgeModel::handle(&app);
        });
    }
}
