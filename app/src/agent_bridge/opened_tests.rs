use std::collections::HashSet;
use std::time::Duration;

use super::*;

const LIMITS: OpenLimits = OpenLimits {
    per_host: 2,
    per_agent: 3,
};

fn session(id: u64) -> SessionId {
    SessionId::from(id)
}

fn opened(agent_id: &str, alias: &str, now: Instant) -> Opened {
    Opened {
        agent_id: agent_id.to_owned(),
        alias: alias.to_owned(),
        access: Access::ReadOnly,
        elevate: false,
        opened_at: now,
        state: OpenState::Connecting,
    }
}

fn registry(entries: &[(&str, &str, &str)], now: Instant) -> OpenedSessions {
    let mut registry = OpenedSessions::default();
    for (pane, agent_id, alias) in entries {
        registry.register((*pane).to_owned(), opened(agent_id, alias, now));
    }
    registry
}

fn ssh_session(id: u64, host: &str) -> Bootstrapped<'_> {
    Bootstrapped {
        session: session(id),
        is_remote: true,
        ssh_host: Some(host),
        spawning_command: "ssh lab-1",
    }
}

fn local_session(id: u64) -> Bootstrapped<'static> {
    Bootstrapped {
        session: session(id),
        is_remote: false,
        ssh_host: None,
        spawning_command: "",
    }
}

fn sudo_session(id: u64) -> Bootstrapped<'static> {
    Bootstrapped {
        session: session(id),
        is_remote: true,
        ssh_host: None,
        spawning_command: "sudo -i",
    }
}

// --- Limits ---------------------------------------------------------------

#[test]
fn an_agent_may_open_sessions_up_to_the_host_and_agent_limits() {
    let now = Instant::now();
    let registry = registry(&[("1", "claude", "lab-1")], now);
    assert_eq!(registry.check_limits("claude", "lab-1", LIMITS), Ok(()));
    assert_eq!(registry.check_limits("claude", "lab-2", LIMITS), Ok(()));
}

#[test]
fn the_per_host_limit_counts_only_that_host_and_that_agent() {
    let now = Instant::now();
    let registry = registry(
        &[
            ("1", "claude", "lab-1"),
            ("2", "claude", "lab-1"),
            ("3", "codex", "lab-2"),
        ],
        now,
    );
    assert_eq!(
        registry.check_limits("claude", "lab-1", LIMITS),
        Err(LimitError::PerHost {
            alias: "lab-1".to_owned(),
            limit: 2
        })
    );
    assert_eq!(registry.check_limits("claude", "lab-2", LIMITS), Ok(()));
    assert_eq!(registry.check_limits("codex", "lab-1", LIMITS), Ok(()));
}

#[test]
fn the_per_agent_limit_counts_every_host() {
    let now = Instant::now();
    let registry = registry(
        &[
            ("1", "claude", "lab-1"),
            ("2", "claude", "lab-2"),
            ("3", "claude", "lab-3"),
        ],
        now,
    );
    assert_eq!(
        registry.check_limits("claude", "lab-4", LIMITS),
        Err(LimitError::PerAgent { limit: 3 })
    );
    assert_eq!(registry.check_limits("codex", "lab-4", LIMITS), Ok(()));
}

#[test]
fn sessions_that_never_became_ready_still_count() {
    let now = Instant::now();
    let mut registry = registry(&[("1", "claude", "lab-1"), ("2", "claude", "lab-1")], now);
    registry.expire(now + OPEN_PENDING_TTL + Duration::from_secs(1));
    assert_eq!(
        registry.get("1").map(|opened| opened.state),
        Some(OpenState::Abandoned)
    );
    assert!(matches!(
        registry.check_limits("claude", "lab-1", LIMITS),
        Err(LimitError::PerHost { .. })
    ));
}

#[test]
fn all_agents_together_are_capped() {
    let now = Instant::now();
    let mut registry = OpenedSessions::default();
    for index in 0..OPEN_MAX_TOTAL {
        registry.register(
            index.to_string(),
            opened(&format!("agent-{index}"), &format!("host-{index}"), now),
        );
    }
    assert_eq!(
        registry.check_limits("someone-new", "lab-1", LIMITS),
        Err(LimitError::Total {
            limit: OPEN_MAX_TOTAL
        })
    );
}

#[test]
fn limit_messages_tell_the_agent_what_to_do() {
    let per_host = LimitError::PerHost {
        alias: "lab-1".to_owned(),
        limit: 2,
    }
    .to_string();
    assert!(per_host.contains("lab-1") && per_host.contains("close_session"));
    assert!(
        LimitError::PerAgent { limit: 4 }
            .to_string()
            .contains("close_session")
    );
}

#[test]
fn closing_or_losing_the_pane_frees_the_slot() {
    let now = Instant::now();
    let mut registry = registry(
        &[
            ("1", "claude", "lab-1"),
            ("2", "claude", "lab-1"),
            ("3", "claude", "lab-2"),
        ],
        now,
    );
    assert!(registry.remove("1").is_some());
    assert!(registry.remove("1").is_none());
    assert_eq!(registry.check_limits("claude", "lab-1", LIMITS), Ok(()));

    let live: HashSet<String> = ["3".to_owned()].into();
    assert_eq!(registry.retain_live(&live), vec!["2".to_owned()]);
    assert!(registry.get("2").is_none());
    assert!(registry.get("3").is_some());
}

// --- Becoming ready ---------------------------------------------------------

#[test]
fn the_shell_of_the_new_tab_is_not_the_session_that_was_asked_for() {
    let now = Instant::now();
    let mut registry = registry(&[("1", "claude", "lab-1")], now);
    assert_eq!(
        registry.on_bootstrapped("1", local_session(10)),
        Step::Ignore
    );
    assert_eq!(
        registry.get("1").map(|opened| opened.state),
        Some(OpenState::Connecting)
    );
}

#[test]
fn an_ssh_session_to_the_alias_is_attached_with_the_requested_access() {
    let now = Instant::now();
    let mut registry = registry(&[("1", "claude", "lab-1")], now);
    assert_eq!(
        registry.on_bootstrapped("1", ssh_session(11, "lab-1")),
        Step::Attach {
            session: session(11),
            access: Access::ReadOnly,
            replaces: None,
            elevate: false,
        }
    );
    assert_eq!(
        registry.get("1").map(|opened| opened.state),
        Some(OpenState::Ready {
            session: session(11)
        })
    );
}

#[test]
fn the_user_part_of_what_was_typed_after_ssh_is_ignored() {
    let now = Instant::now();
    let mut registry = registry(&[("1", "claude", "lab-1")], now);
    assert!(matches!(
        registry.on_bootstrapped("1", ssh_session(11, "ops@lab-1")),
        Step::Attach { .. }
    ));
}

#[test]
fn an_ssh_session_to_another_host_is_ignored() {
    let now = Instant::now();
    let mut registry = registry(&[("1", "claude", "lab-1")], now);
    assert_eq!(
        registry.on_bootstrapped("1", ssh_session(11, "lab-2")),
        Step::Ignore
    );
    let no_host = Bootstrapped {
        ssh_host: None,
        ..ssh_session(11, "lab-1")
    };
    assert_eq!(registry.on_bootstrapped("1", no_host), Step::Ignore);
}

#[test]
fn a_session_that_is_not_remote_is_ignored_even_with_the_right_host() {
    let now = Instant::now();
    let mut registry = registry(&[("1", "claude", "lab-1")], now);
    let local = Bootstrapped {
        is_remote: false,
        ..ssh_session(11, "lab-1")
    };
    assert_eq!(registry.on_bootstrapped("1", local), Step::Ignore);
}

#[test]
fn a_tab_nobody_opened_is_ignored() {
    let mut registry = OpenedSessions::default();
    assert_eq!(
        registry.on_bootstrapped("1", ssh_session(11, "lab-1")),
        Step::Ignore
    );
}

#[test]
fn a_session_that_is_ready_is_not_attached_again() {
    let now = Instant::now();
    let mut registry = registry(&[("1", "claude", "lab-1")], now);
    registry.on_bootstrapped("1", ssh_session(11, "lab-1"));
    assert_eq!(
        registry.on_bootstrapped("1", ssh_session(12, "lab-1")),
        Step::Ignore
    );
    assert_eq!(
        registry.on_bootstrapped("1", sudo_session(13)),
        Step::Ignore
    );
}

#[test]
fn a_late_session_still_attaches_until_the_ttl_runs_out() {
    let now = Instant::now();
    let mut registry = registry(&[("1", "claude", "lab-1")], now);
    registry.expire(now + OPEN_PENDING_TTL);
    assert!(matches!(
        registry.on_bootstrapped("1", ssh_session(11, "lab-1")),
        Step::Attach { .. }
    ));

    let mut late = registry_with_one(now);
    late.expire(now + OPEN_PENDING_TTL + Duration::from_secs(1));
    assert_eq!(
        late.on_bootstrapped("1", ssh_session(11, "lab-1")),
        Step::Ignore
    );
}

fn registry_with_one(now: Instant) -> OpenedSessions {
    registry(&[("1", "claude", "lab-1")], now)
}

// --- Becoming root ------------------------------------------------------------

fn eliciting_root(now: Instant) -> OpenedSessions {
    let mut registry = OpenedSessions::default();
    registry.register(
        "1".to_owned(),
        Opened {
            elevate: true,
            access: Access::Full,
            ..opened("claude", "lab-1", now)
        },
    );
    registry
}

#[test]
fn asking_for_root_attaches_the_login_user_and_waits_for_the_root_shell() {
    let now = Instant::now();
    let mut registry = eliciting_root(now);
    assert_eq!(
        registry.on_bootstrapped("1", ssh_session(11, "lab-1")),
        Step::Attach {
            session: session(11),
            access: Access::Full,
            replaces: None,
            elevate: true,
        }
    );
    assert_eq!(
        registry.get("1").map(|opened| opened.state),
        Some(OpenState::Elevating {
            user_session: session(11)
        })
    );
}

#[test]
fn the_root_shell_takes_over_from_the_login_users_session() {
    let now = Instant::now();
    let mut registry = eliciting_root(now);
    registry.on_bootstrapped("1", ssh_session(11, "lab-1"));
    assert_eq!(
        registry.on_bootstrapped("1", sudo_session(12)),
        Step::Attach {
            session: session(12),
            access: Access::Full,
            replaces: Some(session(11)),
            elevate: false,
        }
    );
    assert_eq!(
        registry.get("1").map(|opened| opened.state),
        Some(OpenState::Ready {
            session: session(12)
        })
    );
}

#[test]
fn only_the_sudo_shell_counts_as_the_root_shell() {
    let now = Instant::now();
    let mut registry = eliciting_root(now);
    registry.on_bootstrapped("1", ssh_session(11, "lab-1"));
    let other = Bootstrapped {
        spawning_command: "bash",
        ..sudo_session(12)
    };
    assert_eq!(registry.on_bootstrapped("1", other), Step::Ignore);
    let not_remote = Bootstrapped {
        is_remote: false,
        ..sudo_session(12)
    };
    assert_eq!(registry.on_bootstrapped("1", not_remote), Step::Ignore);
}

#[test]
fn a_root_shell_that_never_comes_leaves_the_login_users_session() {
    let now = Instant::now();
    let mut registry = eliciting_root(now);
    registry.on_bootstrapped("1", ssh_session(11, "lab-1"));
    registry.expire(now + OPEN_PENDING_TTL + Duration::from_secs(1));
    assert_eq!(
        registry.get("1").map(|opened| opened.state),
        Some(OpenState::Ready {
            session: session(11)
        })
    );
}

#[test]
fn expiry_leaves_ready_sessions_alone() {
    let now = Instant::now();
    let mut registry = registry_with_one(now);
    registry.on_bootstrapped("1", ssh_session(11, "lab-1"));
    registry.expire(now + OPEN_PENDING_TTL * 3);
    assert_eq!(
        registry.get("1").map(|opened| opened.state),
        Some(OpenState::Ready {
            session: session(11)
        })
    );
}

// --- Deciding about root -------------------------------------------------------

#[test]
fn nothing_is_done_about_root_unless_it_was_asked_for() {
    for login in [
        RootLogin::Root,
        RootLogin::SudoNopasswd,
        RootLogin::SudoPassword,
        RootLogin::None,
    ] {
        assert_eq!(
            elevation_plan(false, login, true),
            ElevationPlan::NotRequested
        );
    }
}

#[test]
fn a_root_login_needs_no_sudo() {
    assert_eq!(
        elevation_plan(true, RootLogin::Root, false),
        ElevationPlan::AlreadyRoot
    );
}

#[test]
fn sudo_without_a_password_runs_when_warpify_would_take_the_command() {
    assert_eq!(
        elevation_plan(true, RootLogin::SudoNopasswd, true),
        ElevationPlan::Run
    );
}

#[test]
fn sudo_that_warpify_would_not_take_stays_as_the_login_user_and_says_how_to_fix_it() {
    let ElevationPlan::Skipped(reason) = elevation_plan(true, RootLogin::SudoNopasswd, false)
    else {
        panic!("expected the plan to be skipped");
    };
    assert!(reason.contains(SUDO_COMMAND) && reason.contains("Warpify"));
}

#[test]
fn servers_that_need_a_password_or_have_no_way_to_root_are_not_elevated() {
    for login in [RootLogin::SudoPassword, RootLogin::None] {
        assert!(matches!(
            elevation_plan(true, login, true),
            ElevationPlan::Skipped(_)
        ));
    }
}
