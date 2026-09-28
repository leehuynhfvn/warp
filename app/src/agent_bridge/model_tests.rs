use std::time::{Duration, SystemTime};

use futures::executor::block_on;
use warpui::{App, ModelHandle, WindowId};

use super::*;
use crate::agent_bridge::approval::{AgentLabel, ApprovalDecision, ApprovalRequest, ApprovalSubject};

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
