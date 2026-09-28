use std::time::{Duration, SystemTime};

use futures::executor::block_on;
use warpui::WindowId;

use super::*;

fn session(id: u64) -> SessionId {
    SessionId::from(id)
}

fn window(id: usize) -> WindowId {
    WindowId::from_usize(id)
}

fn request(session: SessionId, window_id: WindowId) -> ApprovalRequest {
    ApprovalRequest {
        request_id: Uuid::new_v4(),
        session,
        session_label: "root@lab-1".to_owned(),
        agent: Some("claude-code".to_owned()),
        subject: ApprovalSubject::Command {
            command: "uptime".to_owned(),
            cwd: None,
            visible: false,
        },
        deadline: SystemTime::now() + Duration::from_secs(300),
        window_id,
    }
}

// --- push / decide / remove -------------------------------------------------

#[test]
fn decide_delivers_the_decision_and_removes_the_request() {
    let mut queue = ApprovalQueue::default();
    let req = request(session(1), window(1));
    let id = req.request_id;
    let receiver = queue.push(req).expect("queue has room");

    assert!(queue.decide(id, ApprovalDecision::Approve));
    assert_eq!(
        block_on(receiver),
        Ok(ApprovalDecision::Approve),
        "the waiting receiver should get the decision"
    );
    assert!(
        queue.get(id).is_none(),
        "a decided request leaves the queue"
    );
}

#[test]
fn decide_on_an_unknown_request_returns_false() {
    let mut queue = ApprovalQueue::default();
    assert!(!queue.decide(Uuid::new_v4(), ApprovalDecision::Deny));
}

#[test]
fn decide_after_the_request_was_already_removed_returns_false() {
    let mut queue = ApprovalQueue::default();
    let req = request(session(1), window(1));
    let id = req.request_id;
    let _receiver = queue.push(req).expect("queue has room");

    assert!(queue.remove(id), "the request is still in the queue");
    assert!(
        !queue.decide(id, ApprovalDecision::Approve),
        "a decision arriving after the wait already ended (mục 3.4's timeout race) is not an \
         error, just a no-op"
    );
}

#[test]
fn remove_drops_the_request_without_sending_a_decision() {
    let mut queue = ApprovalQueue::default();
    let req = request(session(1), window(1));
    let id = req.request_id;
    let receiver = queue.push(req).expect("queue has room");

    assert!(queue.remove(id));
    assert!(
        block_on(receiver).is_err(),
        "removing without deciding drops the sender, so the receiver sees Canceled"
    );
}

#[test]
fn remove_on_an_unknown_request_returns_false() {
    let mut queue = ApprovalQueue::default();
    assert!(!queue.remove(Uuid::new_v4()));
}

#[test]
fn remove_after_a_decision_already_sent_returns_false() {
    let mut queue = ApprovalQueue::default();
    let req = request(session(1), window(1));
    let id = req.request_id;
    let _receiver = queue.push(req).expect("queue has room");

    assert!(queue.decide(id, ApprovalDecision::Deny));
    assert!(
        !queue.remove(id),
        "the timeout-race cleanup in mục 3.4 is a no-op once a decision already won"
    );
}

// --- capacity ----------------------------------------------------------------

#[test]
fn a_session_may_not_have_more_than_the_configured_number_of_pending_requests() {
    let mut queue = ApprovalQueue::default();
    let target = session(1);
    for _ in 0..MAX_PENDING_APPROVALS_PER_SESSION {
        drop(
            queue
                .push(request(target, window(1)))
                .expect("under the limit"),
        );
    }

    let error = queue
        .push(request(target, window(1)))
        .expect_err("the limit was reached");
    assert!(matches!(error, AgentBridgeError::PolicyDenied(reason)
        if reason.contains("too many requests")));
}

#[test]
fn the_per_session_limit_does_not_affect_other_sessions() {
    let mut queue = ApprovalQueue::default();
    for _ in 0..MAX_PENDING_APPROVALS_PER_SESSION {
        drop(
            queue
                .push(request(session(1), window(1)))
                .expect("under the limit"),
        );
    }
    drop(
        queue
            .push(request(session(2), window(1)))
            .expect("a different session has its own budget"),
    );
}

// --- revoke --------------------------------------------------------------

#[test]
fn revoke_session_denies_and_removes_only_that_sessions_requests() {
    let mut queue = ApprovalQueue::default();
    let a = request(session(1), window(1));
    let b = request(session(1), window(1));
    let c = request(session(2), window(1));
    let (a_id, b_id, c_id) = (a.request_id, b.request_id, c.request_id);
    let a_rx = queue.push(a).expect("queue has room");
    let b_rx = queue.push(b).expect("queue has room");
    let _c_rx = queue.push(c).expect("queue has room");

    assert_eq!(queue.revoke_session(session(1)), 2);
    assert_eq!(block_on(a_rx), Ok(ApprovalDecision::Revoked));
    assert_eq!(block_on(b_rx), Ok(ApprovalDecision::Revoked));
    assert!(queue.get(a_id).is_none());
    assert!(queue.get(b_id).is_none());
    assert!(
        queue.get(c_id).is_some(),
        "a different session is untouched"
    );
}

#[test]
fn revoke_session_with_nothing_pending_returns_zero() {
    let mut queue = ApprovalQueue::default();
    assert_eq!(queue.revoke_session(session(1)), 0);
}

#[test]
fn revoke_all_denies_and_removes_every_request() {
    let mut queue = ApprovalQueue::default();
    let a_rx = queue
        .push(request(session(1), window(1)))
        .expect("queue has room");
    let b_rx = queue
        .push(request(session(2), window(2)))
        .expect("queue has room");

    assert_eq!(queue.revoke_all(), 2);
    assert_eq!(block_on(a_rx), Ok(ApprovalDecision::Revoked));
    assert_eq!(block_on(b_rx), Ok(ApprovalDecision::Revoked));
    assert_eq!(queue.count_for_session(session(1)), 0);
    assert_eq!(queue.count_for_session(session(2)), 0);
}

// --- lookups ---------------------------------------------------------------

#[test]
fn get_finds_a_pending_request_by_id() {
    let mut queue = ApprovalQueue::default();
    let req = request(session(1), window(1));
    let id = req.request_id;
    let _receiver = queue.push(req).expect("queue has room");

    assert_eq!(queue.get(id).map(|request| request.request_id), Some(id));
    assert!(queue.get(Uuid::new_v4()).is_none());
}

#[test]
fn count_for_session_only_counts_that_session() {
    let mut queue = ApprovalQueue::default();
    drop(
        queue
            .push(request(session(1), window(1)))
            .expect("queue has room"),
    );
    drop(
        queue
            .push(request(session(1), window(1)))
            .expect("queue has room"),
    );
    drop(
        queue
            .push(request(session(2), window(1)))
            .expect("queue has room"),
    );

    assert_eq!(queue.count_for_session(session(1)), 2);
    assert_eq!(queue.count_for_session(session(2)), 1);
    assert_eq!(queue.count_for_session(session(3)), 0);
}

#[test]
fn oldest_for_session_returns_the_first_one_pushed() {
    let mut queue = ApprovalQueue::default();
    let first = request(session(1), window(1));
    let first_id = first.request_id;
    drop(queue.push(first).expect("queue has room"));
    drop(
        queue
            .push(request(session(1), window(1)))
            .expect("queue has room"),
    );

    assert_eq!(
        queue.oldest_for_session(session(1)).map(|r| r.request_id),
        Some(first_id)
    );
    assert!(queue.oldest_for_session(session(2)).is_none());
}

#[test]
fn oldest_in_window_returns_the_first_one_pushed_in_that_window() {
    let mut queue = ApprovalQueue::default();
    let first = request(session(1), window(1));
    let first_id = first.request_id;
    drop(queue.push(first).expect("queue has room"));
    drop(
        queue
            .push(request(session(2), window(2)))
            .expect("queue has room"),
    );

    assert_eq!(
        queue.oldest_in_window(window(1)).map(|r| r.request_id),
        Some(first_id)
    );
    assert!(queue.oldest_in_window(window(3)).is_none());
}

// --- wait_for_decision -------------------------------------------------------

#[test]
fn wait_for_decision_returns_a_decision_that_arrives_before_the_timeout() {
    let mut queue = ApprovalQueue::default();
    let req = request(session(1), window(1));
    let id = req.request_id;
    let receiver = queue.push(req).expect("queue has room");

    assert!(queue.decide(id, ApprovalDecision::AllowInSession));
    let decision = block_on(wait_for_decision(receiver, Duration::from_secs(30)));
    assert_eq!(decision, ApprovalDecision::AllowInSession);
}

#[test]
fn wait_for_decision_times_out_when_nobody_decides() {
    let mut queue = ApprovalQueue::default();
    let req = request(session(1), window(1));
    let receiver = queue.push(req).expect("queue has room");

    let decision = block_on(wait_for_decision(receiver, Duration::from_millis(10)));
    assert_eq!(decision, ApprovalDecision::TimedOut);
}

#[test]
fn wait_for_decision_treats_a_dropped_sender_as_revoked() {
    let (sender, receiver) = futures::channel::oneshot::channel::<ApprovalDecision>();
    drop(sender);

    let decision = block_on(wait_for_decision(receiver, Duration::from_secs(30)));
    assert_eq!(decision, ApprovalDecision::Revoked);
}

#[test]
fn a_decision_that_wins_the_race_against_the_timeout_still_leaves_the_queue_consistent() {
    // Mirrors mục 3.4's documented race: the timer fires and the caller starts cleaning up with
    // `remove`, but a decision reaches the queue microseconds earlier. `decide` wins (the request
    // leaves the queue as decided), and the later `remove` is a harmless no-op.
    let mut queue = ApprovalQueue::default();
    let req = request(session(1), window(1));
    let id = req.request_id;
    let _receiver = queue.push(req).expect("queue has room");

    assert!(queue.decide(id, ApprovalDecision::Deny));
    assert!(!queue.remove(id));
}
