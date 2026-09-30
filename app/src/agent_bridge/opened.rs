//! The sessions agents opened themselves (`remote.session.open`), kept only in memory like the
//! attachments they lead to. Pure: what happens when a session finishes Warpifying, who may close
//! a session, and how many an agent may hold are decided here from values passed in, so none of it
//! needs a terminal.

use std::collections::{HashMap, HashSet};

use instant::Instant;

use super::attachments::Access;
use super::policy::OpenLimits;
use super::{OPEN_MAX_TOTAL, OPEN_PENDING_TTL};
use crate::host_directory::{RootLogin, alias_of_ssh_host};
use crate::terminal::model::session::SessionId;

/// The exact command that becomes root. It has to match one of Warpify's subshell commands, so
/// what is typed and what is checked can never differ.
pub(crate) const SUDO_COMMAND: &str = "sudo -i";

/// Where an opened session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenState {
    /// The tab is open and its `ssh` has not Warpified yet.
    Connecting,
    /// Signed in as the login user, with [`SUDO_COMMAND`] typed and its shell not ready yet.
    Elevating {
        user_session: SessionId,
    },
    Ready {
        session: SessionId,
    },
    /// Did not become ready in time. It is not attached any more if it does, but the tab is still
    /// open, so it counts against the limits until it is closed.
    Abandoned,
}

/// What kind of root shell the request should end in, decided before anything is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ElevationPlan {
    NotRequested,
    /// The SSH login is root already.
    AlreadyRoot,
    /// Type [`SUDO_COMMAND`] once signed in.
    Run,
    /// Stay as the login user; the text says why, for the agent.
    Skipped(String),
}

/// Works out what to do about `root: true` from what the server directory knows about the server
/// and whether Warpify would take [`SUDO_COMMAND`] as a subshell.
pub(crate) fn elevation_plan(
    want_root: bool,
    root_login: RootLogin,
    sudo_is_warpifiable: bool,
) -> ElevationPlan {
    if !want_root {
        return ElevationPlan::NotRequested;
    }
    match root_login {
        RootLogin::Root => ElevationPlan::AlreadyRoot,
        RootLogin::SudoNopasswd if sudo_is_warpifiable => ElevationPlan::Run,
        RootLogin::SudoNopasswd => ElevationPlan::Skipped(format!(
            "'{SUDO_COMMAND}' is not one of Warpify's subshell commands, so the root shell would \
             not be Warpified. Add it in Settings > Warpify > Added commands."
        )),
        RootLogin::SudoPassword => ElevationPlan::Skipped(
            "this server needs a sudo password, which agents cannot use yet.".to_owned(),
        ),
        RootLogin::None => ElevationPlan::Skipped(
            "the server's entry says there is no way to become root.".to_owned(),
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Opened {
    /// The paired identity that opened it; only this agent may close it.
    pub(crate) agent_id: String,
    pub(crate) alias: String,
    pub(crate) access: Access,
    pub(crate) elevate: bool,
    pub(crate) opened_at: Instant,
    pub(crate) state: OpenState,
}

/// A session that finished Warpifying in a tab an agent opened.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bootstrapped<'a> {
    pub(crate) session: SessionId,
    pub(crate) is_remote: bool,
    /// What the user typed after `ssh`, when the session is an SSH one.
    pub(crate) ssh_host: Option<&'a str>,
    pub(crate) spawning_command: &'a str,
}

/// What to do about a [`Bootstrapped`] session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Ignore,
    /// Attach `session` with `access`. `replaces` is the login user's session that the root one
    /// took over from, to be detached. `elevate` says `sudo -i` still has to be typed, so the
    /// request is not answered yet.
    Attach {
        session: SessionId,
        access: Access,
        replaces: Option<SessionId>,
        elevate: bool,
    },
}

/// Why a session may not be opened, for the agent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LimitError {
    #[error(
        "you already have {limit} sessions open on {alias}; close one with close_session first."
    )]
    PerHost { alias: String, limit: usize },
    #[error("you already have {limit} sessions open; close one with close_session first.")]
    PerAgent { limit: usize },
    #[error("too many sessions opened by agents are open at once ({limit}).")]
    Total { limit: usize },
}

#[derive(Debug, Default)]
pub(crate) struct OpenedSessions {
    /// By the id clients use for the session, which is its pane's.
    by_pane: HashMap<String, Opened>,
}

impl OpenedSessions {
    /// Whether `agent_id` may open one more session on `alias`.
    pub(crate) fn check_limits(
        &self,
        agent_id: &str,
        alias: &str,
        limits: OpenLimits,
    ) -> Result<(), LimitError> {
        if self.by_pane.len() >= OPEN_MAX_TOTAL {
            return Err(LimitError::Total {
                limit: OPEN_MAX_TOTAL,
            });
        }
        let own = || {
            self.by_pane
                .values()
                .filter(|opened| opened.agent_id == agent_id)
        };
        if own().count() >= limits.per_agent {
            return Err(LimitError::PerAgent {
                limit: limits.per_agent,
            });
        }
        if own().filter(|opened| opened.alias == alias).count() >= limits.per_host {
            return Err(LimitError::PerHost {
                alias: alias.to_owned(),
                limit: limits.per_host,
            });
        }
        Ok(())
    }

    pub(crate) fn register(&mut self, pane: String, opened: Opened) {
        self.by_pane.insert(pane, opened);
    }

    pub(crate) fn get(&self, pane: &str) -> Option<&Opened> {
        self.by_pane.get(pane)
    }

    pub(crate) fn remove(&mut self, pane: &str) -> Option<Opened> {
        self.by_pane.remove(pane)
    }

    /// Forgets sessions whose pane is gone (the user closed the tab), so they stop counting.
    /// Returns the panes that were forgotten.
    pub(crate) fn retain_live(&mut self, live_panes: &HashSet<String>) -> Vec<String> {
        let gone: Vec<String> = self
            .by_pane
            .keys()
            .filter(|pane| !live_panes.contains(*pane))
            .cloned()
            .collect();
        for pane in &gone {
            self.by_pane.remove(pane);
        }
        gone
    }

    /// Gives up on sessions that have been waiting to become ready for too long: one still
    /// connecting is abandoned, one still elevating stays as the login user's session.
    pub(crate) fn expire(&mut self, now: Instant) {
        for opened in self.by_pane.values_mut() {
            if now.saturating_duration_since(opened.opened_at) <= OPEN_PENDING_TTL {
                continue;
            }
            opened.state = match opened.state {
                OpenState::Connecting => OpenState::Abandoned,
                OpenState::Elevating { user_session } => OpenState::Ready {
                    session: user_session,
                },
                state @ (OpenState::Ready { .. } | OpenState::Abandoned) => state,
            };
        }
    }

    /// Keeps the login user's session as the one that is ready, when the root shell could not be
    /// started. Returns that session.
    pub(crate) fn give_up_elevation(&mut self, pane: &str) -> Option<SessionId> {
        let opened = self.by_pane.get_mut(pane)?;
        let OpenState::Elevating { user_session } = opened.state else {
            return None;
        };
        opened.state = OpenState::Ready {
            session: user_session,
        };
        Some(user_session)
    }

    /// Moves the session of `pane` along when `event` finishes Warpifying in its tab.
    pub(crate) fn on_bootstrapped(&mut self, pane: &str, event: Bootstrapped<'_>) -> Step {
        let Some(opened) = self.by_pane.get_mut(pane) else {
            return Step::Ignore;
        };
        match opened.state {
            OpenState::Connecting => {
                let reached_alias = event.is_remote
                    && event
                        .ssh_host
                        .is_some_and(|host| alias_of_ssh_host(host) == opened.alias);
                if !reached_alias {
                    return Step::Ignore;
                }
                if opened.elevate {
                    opened.state = OpenState::Elevating {
                        user_session: event.session,
                    };
                } else {
                    opened.state = OpenState::Ready {
                        session: event.session,
                    };
                }
                Step::Attach {
                    session: event.session,
                    access: opened.access,
                    replaces: None,
                    elevate: opened.elevate,
                }
            }
            OpenState::Elevating { user_session } => {
                if !event.is_remote || event.spawning_command.trim() != SUDO_COMMAND {
                    return Step::Ignore;
                }
                opened.state = OpenState::Ready {
                    session: event.session,
                };
                Step::Attach {
                    session: event.session,
                    access: opened.access,
                    replaces: Some(user_session),
                    elevate: false,
                }
            }
            OpenState::Ready { .. } | OpenState::Abandoned => Step::Ignore,
        }
    }
}

#[cfg(test)]
#[path = "opened_tests.rs"]
mod tests;
