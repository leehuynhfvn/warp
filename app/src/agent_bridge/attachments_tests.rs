use std::time::Duration;

use instant::Instant;

use super::*;

fn session(id: u64) -> SessionId {
    SessionId::from(id)
}

fn attached(access: Access, now: Instant) -> Attachments {
    let mut attachments = Attachments::default();
    attachments.attach(session(1), access, "root".to_owned(), "prod-1".to_owned(), now);
    attachments
}

fn check(
    attachments: &mut Attachments,
    id: u64,
    needed: Access,
    now: Instant,
) -> Result<Access, AgentBridgeError> {
    attachments
        .check(session(id), needed, "alice", "prod-2", now)
        .map(Attachment::access)
}

#[test]
fn a_full_attachment_allows_every_kind_of_access() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    assert_eq!(check(&mut attachments, 1, Access::ReadOnly, now), Ok(Access::Full));
    assert_eq!(check(&mut attachments, 1, Access::Full, now), Ok(Access::Full));
}

#[test]
fn a_read_only_attachment_refuses_commands_and_writes() {
    let now = Instant::now();
    let mut attachments = attached(Access::ReadOnly, now);
    assert_eq!(check(&mut attachments, 1, Access::ReadOnly, now), Ok(Access::ReadOnly));
    assert_eq!(
        check(&mut attachments, 1, Access::Full, now),
        Err(AgentBridgeError::ReadOnlyAttachment)
    );
    assert!(attachments.get(session(1), now).is_some());
}

#[test]
fn another_session_is_not_attached_and_the_error_names_it() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    assert_eq!(
        check(&mut attachments, 2, Access::ReadOnly, now),
        Err(AgentBridgeError::NotAttached {
            user: "alice".to_owned(),
            host: "prod-2".to_owned()
        })
    );
}

#[test]
fn an_attachment_expires_after_the_idle_period_and_is_removed() {
    let start = Instant::now();
    let mut attachments = attached(Access::Full, start);
    let just_before = start + ATTACH_IDLE_TTL;
    assert!(attachments.get(session(1), just_before).is_some());

    let after = start + ATTACH_IDLE_TTL + Duration::from_secs(1);
    assert!(attachments.get(session(1), after).is_none());
    assert_eq!(
        check(&mut attachments, 1, Access::ReadOnly, after),
        Err(AgentBridgeError::AttachmentExpired)
    );
    assert_eq!(
        check(&mut attachments, 1, Access::ReadOnly, after),
        Err(AgentBridgeError::NotAttached {
            user: "alice".to_owned(),
            host: "prod-2".to_owned()
        })
    );
}

#[test]
fn using_a_session_extends_it_and_counts_only_commands() {
    let start = Instant::now();
    let mut attachments = attached(Access::Full, start);
    let later = start + ATTACH_IDLE_TTL - Duration::from_secs(1);

    attachments.record_use(session(1), true, later);
    attachments.record_use(session(1), false, later);

    let after_original_expiry = start + ATTACH_IDLE_TTL + Duration::from_secs(60);
    let attachment = attachments
        .get(session(1), after_original_expiry)
        .expect("the use restarted the idle timer");
    assert_eq!(attachment.exec_count(), 1);
    assert_eq!(attachment.user(), "root");
    assert_eq!(attachment.host(), "prod-1");
}

#[test]
fn recording_use_of_a_detached_session_does_nothing() {
    let now = Instant::now();
    let mut attachments = Attachments::default();
    attachments.record_use(session(1), true, now);
    assert!(attachments.get(session(1), now).is_none());
}

#[test]
fn attaching_again_replaces_the_access_and_restarts_the_count() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    attachments.record_use(session(1), true, now);
    attachments.attach(session(1), Access::ReadOnly, "root".to_owned(), "prod-1".to_owned(), now);

    let attachment = attachments.get(session(1), now).unwrap();
    assert_eq!(attachment.access(), Access::ReadOnly);
    assert_eq!(attachment.exec_count(), 0);
}

#[test]
fn detaching_removes_one_session_and_reports_whether_it_was_there() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    attachments.attach(session(2), Access::Full, "root".to_owned(), "prod-2".to_owned(), now);

    assert!(attachments.detach(session(1)));
    assert!(!attachments.detach(session(1)));
    assert!(attachments.get(session(2), now).is_some());
}

#[test]
fn detach_all_returns_how_many_sessions_it_removed() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    attachments.attach(session(2), Access::ReadOnly, "root".to_owned(), "prod-2".to_owned(), now);

    assert_eq!(attachments.detach_all(), 2);
    assert_eq!(attachments.detach_all(), 0);
    assert!(attachments.get(session(1), now).is_none());
}
