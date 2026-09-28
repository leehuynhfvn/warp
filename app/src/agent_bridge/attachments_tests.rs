use std::time::Duration;

use instant::Instant;

use super::*;

fn session(id: u64) -> SessionId {
    SessionId::from(id)
}

fn attached(access: Access, now: Instant) -> Attachments {
    let mut attachments = Attachments::default();
    attachments.attach(session(1), access, now);
    attachments
}

fn check(
    attachments: &mut Attachments,
    id: u64,
    needed: Access,
    now: Instant,
) -> Result<Access, AgentBridgeError> {
    attachments.check(session(id), needed, "alice", "prod-2", now)?;
    Ok(attachments
        .status(session(id), now)
        .expect("a checked session is attached")
        .access)
}

#[test]
fn a_full_attachment_allows_every_kind_of_access() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    assert_eq!(
        check(&mut attachments, 1, Access::ReadOnly, now),
        Ok(Access::Full)
    );
    assert_eq!(
        check(&mut attachments, 1, Access::Full, now),
        Ok(Access::Full)
    );
}

#[test]
fn a_read_only_attachment_refuses_commands_and_writes() {
    let now = Instant::now();
    let mut attachments = attached(Access::ReadOnly, now);
    assert_eq!(
        check(&mut attachments, 1, Access::ReadOnly, now),
        Ok(Access::ReadOnly)
    );
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
    let status = attachments
        .status(session(1), after_original_expiry)
        .expect("the use restarted the idle timer");
    assert_eq!(status.exec_count, 1);
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
    attachments.attach(session(1), Access::ReadOnly, now);

    let status = attachments.status(session(1), now).unwrap();
    assert_eq!(status.access, Access::ReadOnly);
    assert_eq!(status.exec_count, 0);
}

#[test]
fn detaching_removes_one_session_and_reports_whether_it_was_there() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    attachments.attach(session(2), Access::Full, now);

    assert!(attachments.detach(session(1)));
    assert!(!attachments.detach(session(1)));
    assert!(attachments.get(session(2), now).is_some());
}

#[test]
fn detach_all_returns_how_many_sessions_it_removed() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    attachments.attach(session(2), Access::ReadOnly, now);

    assert_eq!(attachments.detach_all(), 2);
    assert_eq!(attachments.detach_all(), 0);
    assert!(attachments.get(session(1), now).is_none());
}

#[test]
fn the_status_says_how_long_the_session_has_been_idle_and_what_is_left() {
    let start = Instant::now();
    let attachments = attached(Access::ReadOnly, start);
    let idle = Duration::from_secs(90);

    let status = attachments.status(session(1), start + idle).unwrap();

    assert_eq!(
        status,
        AttachmentStatus {
            access: Access::ReadOnly,
            idle,
            expires_in: ATTACH_IDLE_TTL - idle,
            exec_count: 0,
        }
    );
    let expired = start + ATTACH_IDLE_TTL + Duration::from_secs(1);
    assert_eq!(attachments.status(session(1), expired), None);
    assert_eq!(attachments.status(session(2), start), None);
}

#[test]
fn a_command_allowed_in_session_is_recognized_by_its_trimmed_text() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);

    assert!(!attachments.is_command_allowed(session(1), "uptime", now));
    attachments.allow_command(session(1), "uptime");
    assert!(attachments.is_command_allowed(session(1), "uptime", now));
    assert!(attachments.is_command_allowed(session(1), "  uptime  ", now));
    assert!(!attachments.is_command_allowed(session(1), "uptime -p", now));
}

#[test]
fn allowing_a_command_for_an_unknown_session_does_nothing() {
    let now = Instant::now();
    let mut attachments = Attachments::default();
    attachments.allow_command(session(1), "uptime");
    assert!(!attachments.is_command_allowed(session(1), "uptime", now));
}

#[test]
fn an_allowed_command_stops_being_allowed_once_the_attachment_expires() {
    let start = Instant::now();
    let mut attachments = attached(Access::Full, start);
    attachments.allow_command(session(1), "uptime");

    let expired = start + ATTACH_IDLE_TTL + Duration::from_secs(1);
    assert!(!attachments.is_command_allowed(session(1), "uptime", expired));
}

#[test]
fn re_attaching_a_session_clears_its_previously_allowed_commands() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    attachments.allow_command(session(1), "uptime");

    attachments.attach(session(1), Access::Full, now);
    assert!(!attachments.is_command_allowed(session(1), "uptime", now));
}

#[test]
fn trusting_an_attached_session_is_reported_and_remembered() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);

    assert!(!attachments.is_session_trusted(session(1), now));
    assert!(attachments.trust_session(session(1)));
    assert!(attachments.is_session_trusted(session(1), now));
}

#[test]
fn trusting_an_unknown_session_does_nothing() {
    let now = Instant::now();
    let mut attachments = Attachments::default();

    assert!(!attachments.trust_session(session(1)));
    assert!(!attachments.is_session_trusted(session(1), now));
}

#[test]
fn a_trusted_session_stops_being_trusted_once_the_attachment_expires() {
    let start = Instant::now();
    let mut attachments = attached(Access::Full, start);
    attachments.trust_session(session(1));

    let expired = start + ATTACH_IDLE_TTL + Duration::from_secs(1);
    assert!(!attachments.is_session_trusted(session(1), expired));
}

#[test]
fn re_attaching_a_session_clears_that_it_was_trusted() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    attachments.trust_session(session(1));

    attachments.attach(session(1), Access::Full, now);
    assert!(!attachments.is_session_trusted(session(1), now));
}

#[test]
fn detaching_a_trusted_session_removes_it() {
    let now = Instant::now();
    let mut attachments = attached(Access::Full, now);
    attachments.trust_session(session(1));

    attachments.detach(session(1));
    assert!(!attachments.is_session_trusted(session(1), now));
}
