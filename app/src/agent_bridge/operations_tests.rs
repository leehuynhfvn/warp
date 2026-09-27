use super::*;

fn session(id: u64) -> SessionId {
    SessionId::from(id)
}

#[test]
fn hidden_operations_may_overlap_each_other() {
    let mut operations = Operations::default();
    assert_eq!(operations.begin(session(1), OperationKind::Hidden), Ok(()));
    assert_eq!(operations.begin(session(1), OperationKind::Hidden), Ok(()));
}

#[test]
fn a_visible_command_waits_for_every_hidden_operation() {
    let mut operations = Operations::default();
    operations
        .begin(session(1), OperationKind::Hidden)
        .expect("free");
    operations
        .begin(session(1), OperationKind::Hidden)
        .expect("hidden overlap");
    assert_eq!(
        operations.begin(session(1), OperationKind::Visible),
        Err(AgentBridgeError::OperationRunning)
    );
    operations.end(session(1), OperationKind::Hidden);
    assert_eq!(
        operations.begin(session(1), OperationKind::Visible),
        Err(AgentBridgeError::OperationRunning)
    );
    operations.end(session(1), OperationKind::Hidden);
    assert_eq!(operations.begin(session(1), OperationKind::Visible), Ok(()));
}

#[test]
fn a_visible_command_runs_alone() {
    let mut operations = Operations::default();
    operations
        .begin(session(1), OperationKind::Visible)
        .expect("free");
    for kind in [OperationKind::Hidden, OperationKind::Visible] {
        assert_eq!(
            operations.begin(session(1), kind),
            Err(AgentBridgeError::OperationRunning)
        );
    }
    operations.end(session(1), OperationKind::Visible);
    assert_eq!(operations.begin(session(1), OperationKind::Hidden), Ok(()));
}

#[test]
fn sessions_do_not_block_each_other() {
    let mut operations = Operations::default();
    operations
        .begin(session(1), OperationKind::Visible)
        .expect("free");
    assert_eq!(operations.begin(session(2), OperationKind::Visible), Ok(()));
    assert_eq!(operations.begin(session(3), OperationKind::Hidden), Ok(()));
}

#[test]
fn extra_ends_do_not_free_a_running_operation() {
    let mut operations = Operations::default();
    operations.end(session(1), OperationKind::Hidden);
    operations.end(session(1), OperationKind::Visible);
    operations
        .begin(session(1), OperationKind::Hidden)
        .expect("free");
    operations.end(session(1), OperationKind::Hidden);
    operations.end(session(1), OperationKind::Hidden);
    operations
        .begin(session(1), OperationKind::Hidden)
        .expect("free again");
    assert_eq!(
        operations.begin(session(1), OperationKind::Visible),
        Err(AgentBridgeError::OperationRunning)
    );
}

#[test]
fn a_rejected_begin_leaves_no_trace() {
    let mut operations = Operations::default();
    operations
        .begin(session(1), OperationKind::Visible)
        .expect("free");
    assert!(operations.begin(session(1), OperationKind::Hidden).is_err());
    operations.end(session(1), OperationKind::Visible);
    assert!(operations.by_session.is_empty());
}
