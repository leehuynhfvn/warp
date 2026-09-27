use super::*;

fn code_of(error: AgentBridgeError) -> ErrorCode {
    ControlError::from(error).code
}

#[test]
fn every_error_maps_to_the_documented_code() {
    let cases = [
        (
            AgentBridgeError::NotRemoteSession,
            ErrorCode::InvalidSelector,
        ),
        (
            AgentBridgeError::UnsupportedShell,
            ErrorCode::InvalidSelector,
        ),
        (
            AgentBridgeError::NotAttached {
                user: "root".to_owned(),
                host: "prod-1".to_owned(),
            },
            ErrorCode::SessionNotAttached,
        ),
        (
            AgentBridgeError::AttachmentExpired,
            ErrorCode::SessionNotAttached,
        ),
        (
            AgentBridgeError::ReadOnlyAttachment,
            ErrorCode::InsufficientPermissions,
        ),
        (AgentBridgeError::SessionBusy, ErrorCode::SessionBusy),
        (
            AgentBridgeError::OperationRunning,
            ErrorCode::SessionBusy,
        ),
        (AgentBridgeError::Timeout { secs: 5 }, ErrorCode::Timeout),
        (
            AgentBridgeError::InvalidParams("x".to_owned()),
            ErrorCode::InvalidParams,
        ),
        (
            AgentBridgeError::Conflict("x".to_owned()),
            ErrorCode::TargetStateConflict,
        ),
        (
            AgentBridgeError::RemoteFailed("x".to_owned()),
            ErrorCode::RemoteOperationFailed,
        ),
        (
            AgentBridgeError::Executor("x".to_owned()),
            ErrorCode::RemoteOperationFailed,
        ),
        (
            AgentBridgeError::UnexpectedOutput("x".to_owned()),
            ErrorCode::RemoteOperationFailed,
        ),
        (
            AgentBridgeError::Io("x".to_owned()),
            ErrorCode::RemoteOperationFailed,
        ),
    ];
    for (error, code) in cases {
        assert_eq!(code_of(error.clone()), code, "{error:?}");
    }
}

#[test]
fn a_not_attached_message_names_the_session_and_tells_the_user_what_to_do() {
    let error = ControlError::from(AgentBridgeError::NotAttached {
        user: "root".to_owned(),
        host: "prod-1".to_owned(),
    });
    assert!(error.message.contains("root@prod-1"));
    assert!(
        error
            .message
            .contains("Agent Bridge: Allow agents to control this session")
    );
}

#[test]
fn control_messages_do_not_carry_terminal_escape_sequences() {
    let error = ControlError::from(AgentBridgeError::RemoteFailed(
        "bad \u{1b}[31mred".to_owned(),
    ));
    assert!(!error.message.contains('\u{1b}'));
}

#[test]
fn sync_errors_keep_their_meaning_across_the_conversion() {
    assert_eq!(
        AgentBridgeError::from(WarpSyncError::InvalidPath("~ is not allowed".to_owned())),
        AgentBridgeError::InvalidParams("~ is not allowed".to_owned())
    );
    assert_eq!(
        AgentBridgeError::from(WarpSyncError::Timeout),
        AgentBridgeError::Timeout {
            secs: COMMAND_TIMEOUT.as_secs()
        }
    );
    assert_eq!(
        AgentBridgeError::from(WarpSyncError::NotRemoteSession),
        AgentBridgeError::NotRemoteSession
    );
    assert!(matches!(
        AgentBridgeError::from(WarpSyncError::TooManyPending),
        AgentBridgeError::RemoteFailed(_)
    ));
}
