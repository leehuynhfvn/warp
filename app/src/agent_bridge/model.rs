use std::collections::{HashMap, HashSet};

use futures::channel::oneshot;
use instant::Instant;
use uuid::Uuid;
use warpui::{
    Entity, ModelContext, ModelHandle, SingletonEntity, ViewHandle, WeakViewHandle, WindowId,
};

use super::approval::{ApprovalDecision, ApprovalQueue, ApprovalRequest};
use super::attachments::{Access, AttachmentStatus, Attachments};
use super::error::AgentBridgeError;
use super::opened::{
    Bootstrapped, LimitError, OpenState, Opened, OpenedSessions, SUDO_COMMAND, Step, typed_ssh_host,
};
use super::operations::{OperationKind, Operations};
use super::policy::OpenLimits;
use crate::terminal::model::session::{
    BootstrapSessionType, SessionBootstrappedEvent, SessionId, Sessions, SessionsEvent,
};
use crate::terminal::shell::ShellType;
use crate::terminal::view::TerminalView;

/// Which sessions the user has allowed agents to control, and what is waiting for a person to
/// decide on in them. Lives only in memory, so restarting Warp revokes everything and denies
/// every pending request.
#[derive(Default)]
pub struct AgentBridgeModel {
    attachments: Attachments,
    operations: Operations,
    approvals: ApprovalQueue,
    opened: OpenedSessions,
    /// The requests that opened a session and are still waiting for it to be ready, by pane.
    open_waiters: HashMap<String, oneshot::Sender<OpenReady>>,
}

/// A session an agent asked to open is ready to be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenReady {
    pub(crate) session: SessionId,
    pub(crate) user: String,
    pub(crate) host: String,
    /// The session is the root shell that `sudo -i` started.
    pub(crate) elevated: bool,
    /// Why the root shell that was going to be started was not: the session is the login user's.
    pub(crate) root_note: Option<String>,
}

/// What an observer of [`AgentBridgeModel`] beyond a plain `notify()` may care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBridgeEvent {
    /// A new request needs a person's attention. `window_id` names the window whose toast should
    /// offer to review it.
    ApprovalRequested {
        request_id: Uuid,
        window_id: WindowId,
    },
    /// The queue changed in some other way (decided, timed out, revoked). An observer with a
    /// dialog open on a request should check whether that request is still there.
    ApprovalsChanged,
}

impl Entity for AgentBridgeModel {
    type Event = AgentBridgeEvent;
}

impl SingletonEntity for AgentBridgeModel {}

impl AgentBridgeModel {
    pub(crate) fn attach(&mut self, id: SessionId, access: Access, ctx: &mut ModelContext<Self>) {
        self.attachments.attach(id, access, Instant::now());
        ctx.notify();
    }

    /// Detaches `id` and denies whatever of its requests were still waiting for a decision (mục
    /// 2.4 of the O2 plan: detach is a kill switch for the queue too).
    pub(crate) fn detach(&mut self, id: SessionId, ctx: &mut ModelContext<Self>) -> bool {
        let was_attached = self.attachments.detach(id);
        let revoked = self.approvals.revoke_session(id);
        if was_attached || revoked > 0 {
            ctx.notify();
        }
        if revoked > 0 {
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        was_attached
    }

    pub(crate) fn detach_all(&mut self, ctx: &mut ModelContext<Self>) -> usize {
        let count = self.attachments.detach_all();
        let revoked = self.approvals.revoke_all();
        if count > 0 || revoked > 0 {
            ctx.notify();
        }
        if revoked > 0 {
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        count
    }

    /// Checks that `id` is attached with at least `needed` access. `user` and `host` name the
    /// session in the error when it is not attached.
    pub(crate) fn check(
        &mut self,
        id: SessionId,
        needed: Access,
        user: &str,
        host: &str,
    ) -> Result<(), AgentBridgeError> {
        self.attachments
            .check(id, needed, user, host, Instant::now())
    }

    pub(crate) fn record_use(&mut self, id: SessionId, is_exec: bool) {
        self.attachments.record_use(id, is_exec, Instant::now());
    }

    pub(crate) fn begin_operation(
        &mut self,
        id: SessionId,
        kind: OperationKind,
    ) -> Result<(), AgentBridgeError> {
        self.operations.begin(id, kind)
    }

    pub(crate) fn end_operation(&mut self, id: SessionId, kind: OperationKind) {
        self.operations.end(id, kind);
    }

    pub(crate) fn status(&self, id: SessionId) -> Option<AttachmentStatus> {
        self.attachments.status(id, Instant::now())
    }

    /// Queues `request` for a person to decide on. Emits [`AgentBridgeEvent::ApprovalRequested`]
    /// so a toast/header can offer to review it.
    pub(crate) fn push_approval(
        &mut self,
        request: ApprovalRequest,
        ctx: &mut ModelContext<Self>,
    ) -> Result<oneshot::Receiver<ApprovalDecision>, AgentBridgeError> {
        let request_id = request.request_id;
        let window_id = request.window_id;
        let receiver = self.approvals.push(request)?;
        ctx.notify();
        ctx.emit(AgentBridgeEvent::ApprovalRequested {
            request_id,
            window_id,
        });
        Ok(receiver)
    }

    /// Sends `decision` to whoever is waiting on `request_id`. `false` if it was no longer in the
    /// queue (already decided, or its wait already timed out).
    // Used starting Task 3.3 (the approval dialog's Approve/Deny buttons); remove this `allow`
    // there.
    #[allow(dead_code)]
    pub(crate) fn decide_approval(
        &mut self,
        request_id: Uuid,
        decision: ApprovalDecision,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let decided = self.approvals.decide(request_id, decision);
        if decided {
            ctx.notify();
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        decided
    }

    /// Drops `request_id` without sending a decision, for cleaning up after a wait that timed out.
    pub(crate) fn remove_approval(
        &mut self,
        request_id: Uuid,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let removed = self.approvals.remove(request_id);
        if removed {
            ctx.notify();
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        removed
    }

    /// Denies every pending request across every session ("Deny all waiting agent requests").
    // Used starting Task 3.5 (the palette entry); remove this `allow` there.
    #[allow(dead_code)]
    pub(crate) fn deny_all_approvals(&mut self, ctx: &mut ModelContext<Self>) -> usize {
        let count = self.approvals.revoke_all();
        if count > 0 {
            ctx.notify();
            ctx.emit(AgentBridgeEvent::ApprovalsChanged);
        }
        count
    }

    pub(crate) fn approval(&self, request_id: Uuid) -> Option<&ApprovalRequest> {
        self.approvals.get(request_id)
    }

    pub(crate) fn pending_approvals_for_session(&self, id: SessionId) -> usize {
        self.approvals.count_for_session(id)
    }

    pub(crate) fn has_pending_pairing(&self) -> bool {
        self.approvals.has_pending_pairing()
    }

    pub(crate) fn has_pending_open(&self) -> bool {
        self.approvals.has_pending_open()
    }

    #[cfg(test)]
    pub(crate) fn pending_open_request_ids(&self) -> Vec<Uuid> {
        self.approvals.pending_open_request_ids()
    }

    pub(crate) fn oldest_approval_for_session(&self, id: SessionId) -> Option<&ApprovalRequest> {
        self.approvals.oldest_for_session(id)
    }

    // Used starting Task 3.5 (the toast); remove this `allow` there.
    #[allow(dead_code)]
    pub(crate) fn oldest_approval_in_window(
        &self,
        window_id: WindowId,
    ) -> Option<&ApprovalRequest> {
        self.approvals.oldest_in_window(window_id)
    }

    /// Records "Allow this command in this session" so `command` (trimmed) runs without asking
    /// again until this attachment ends.
    pub(crate) fn allow_command_in_session(&mut self, id: SessionId, command: &str) {
        self.attachments.allow_command(id, command);
    }

    pub(crate) fn is_command_allowed_in_session(&self, id: SessionId, command: &str) -> bool {
        self.attachments
            .is_command_allowed(id, command, Instant::now())
    }

    /// "Trust this session for the rest of the attachment": every later `Decision::Ask` request of
    /// `id` is allowed without queuing it. Returns `false` (and does not notify) if `id` is not
    /// currently attached.
    pub(crate) fn trust_session(&mut self, id: SessionId, ctx: &mut ModelContext<Self>) -> bool {
        let trusted = self.attachments.trust_session(id);
        if trusted {
            ctx.notify();
        }
        trusted
    }

    pub(crate) fn is_session_trusted(&self, id: SessionId) -> bool {
        self.attachments.is_session_trusted(id, Instant::now())
    }

    /// Whether `agent_id` may open one more session on `alias`. Sessions whose pane is not in
    /// `live_panes` any more (their tab was closed) stop counting first.
    pub(crate) fn check_open_limits(
        &mut self,
        agent_id: &str,
        alias: &str,
        limits: OpenLimits,
        live_panes: &HashSet<String>,
    ) -> Result<(), LimitError> {
        for pane in self.opened.retain_live(live_panes) {
            self.open_waiters.remove(&pane);
        }
        self.opened.expire(Instant::now());
        self.opened.check_limits(agent_id, alias, limits)
    }

    /// The session an agent opened in `pane`, if `agent_id` is the agent that opened it.
    pub(crate) fn opened_by(&self, pane: &str, agent_id: &str) -> Option<&Opened> {
        self.opened
            .get(pane)
            .filter(|opened| opened.agent_id == agent_id)
    }

    /// Starts following the session that the tab of `terminal_view` is about to open: attaches it
    /// when it finishes Warpifying in `sessions` (see [`Step`]) and answers on the returned
    /// receiver once it is ready to use. `terminal_view` is only needed to start the root shell,
    /// and a tab that is gone by then just leaves the session as the login user's.
    pub(crate) fn track_open(
        &mut self,
        pane: String,
        opened: Opened,
        sessions: &ModelHandle<Sessions>,
        terminal_view: Option<WeakViewHandle<TerminalView>>,
        ctx: &mut ModelContext<Self>,
    ) -> oneshot::Receiver<OpenReady> {
        let (sender, receiver) = oneshot::channel();
        self.opened.register(pane.clone(), opened);
        self.open_waiters.insert(pane.clone(), sender);
        ctx.subscribe_to_model(sessions, move |me, sessions, event, ctx| {
            if let SessionsEvent::SessionBootstrapped(event) = event {
                let view = terminal_view.as_ref().and_then(|view| view.upgrade(ctx));
                me.follow_open(&pane, view.as_ref(), &sessions, event, ctx);
            }
        });
        ctx.notify();
        receiver
    }

    /// Stops following the session of `pane`, which is being closed, and takes away what an agent
    /// had of it. Returns what was known about it.
    pub(crate) fn forget_opened(
        &mut self,
        pane: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Option<Opened> {
        self.open_waiters.remove(pane);
        let opened = self.opened.remove(pane)?;
        match opened.state {
            OpenState::Ready { session } => {
                self.detach(session, ctx);
            }
            OpenState::Elevating { user_session } => {
                self.detach(user_session, ctx);
            }
            OpenState::Connecting | OpenState::Abandoned => {}
        }
        Some(opened)
    }

    fn follow_open(
        &mut self,
        pane: &str,
        terminal_view: Option<&ViewHandle<TerminalView>>,
        sessions: &ModelHandle<Sessions>,
        event: &SessionBootstrappedEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        let ssh_host = typed_ssh_host(
            event
                .subshell_info
                .as_ref()
                .and_then(|info| info.ssh_connection_info.as_ref())
                .and_then(|info| info.host.as_deref()),
            &event.spawning_command,
        );
        let ssh_host = ssh_host.as_deref();
        self.opened.expire(Instant::now());
        let is_remote = matches!(event.session_type, BootstrapSessionType::WarpifiedRemote);
        let step = self.opened.on_bootstrapped(
            pane,
            Bootstrapped {
                session: event.session_id,
                is_remote,
                ssh_host,
                spawning_command: &event.spawning_command,
            },
        );
        log::info!(
            "Agent session tab {pane}: session {:?} finished Warpifying (remote: {is_remote}, \
             ssh host: {ssh_host:?}, command: {:?}, expected: {:?}, state now: {:?}), next \
             step: {}",
            event.session_id,
            event.spawning_command,
            self.opened.get(pane).map(|opened| opened.alias.as_str()),
            self.opened.get(pane).map(|opened| &opened.state),
            match step {
                Step::Ignore => "ignore",
                Step::Attach { .. } => "attach",
            },
        );
        let Step::Attach {
            session,
            access,
            replaces,
            elevate,
        } = step
        else {
            return;
        };
        self.attach(session, access, ctx);
        if let Some(replaced) = replaces {
            self.detach(replaced, ctx);
        }
        let Some(bootstrapped) = sessions.as_ref(ctx).get(session) else {
            return;
        };
        let ready = OpenReady {
            session,
            user: bootstrapped.user().to_owned(),
            host: bootstrapped.hostname().to_owned(),
            elevated: replaces.is_some(),
            root_note: None,
        };
        if !elevate {
            self.notify_open_ready(pane, ready);
            return;
        }
        let shell_type = event.shell.shell_type();
        let sent = match (terminal_view, shell_type) {
            (_, ShellType::PowerShell) => Err("the server's shell is PowerShell".to_owned()),
            (None, _) => Err("the tab was closed".to_owned()),
            (Some(view), ShellType::Zsh | ShellType::Bash | ShellType::Fish) => {
                let sent = view.update(ctx, |view, ctx| {
                    view.execute_subshell_command_and_warpify(SUDO_COMMAND, shell_type, ctx)
                });
                if sent {
                    Ok(())
                } else {
                    Err("the shell was busy".to_owned())
                }
            }
        };
        if let Err(reason) = sent {
            self.opened.give_up_elevation(pane);
            self.notify_open_ready(
                pane,
                OpenReady {
                    root_note: Some(format!(
                        "'{SUDO_COMMAND}' was not run because {reason}; the session stays as the \
                         login user."
                    )),
                    ..ready
                },
            );
        }
    }

    fn notify_open_ready(&mut self, pane: &str, ready: OpenReady) {
        if let Some(sender) = self.open_waiters.remove(pane)
            && sender.send(ready).is_err()
        {
            log::debug!("An agent stopped waiting for the session it opened");
        }
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
