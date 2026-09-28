//! The approval dialog: shown when a person clicks Review on a request the agent-ops policy is
//! asking about. It is a window onto [`AgentBridgeModel`]'s queue, not an owner of the request
//! itself (mục 3.7 P4 of the O2 plan) — opening a different request while one is already showing
//! never loses the first one, since both still live in the model.

use std::time::SystemTime;

use chrono::{DateTime, Local};
use pathfinder_geometry::vector::vec2f;
use uuid::Uuid;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    Align, ChildAnchor, ChildView, Container, OffsetPositioning, ParentAnchor, ParentOffsetBounds,
    Stack,
};
use warpui::keymap::FixedBinding;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
};

use super::approval::{AgentLabel, ApprovalDecision, ApprovalRequest, ApprovalSubject};
use super::model::AgentBridgeModel;
use crate::appearance::Appearance;
use crate::ui_components::dialog::{Dialog, dialog_styles};
use crate::view_components::action_button::{
    ActionButton, DangerPrimaryTheme, NakedTheme, SecondaryTheme,
};
use crate::warp_sync::printable;

const DIALOG_WIDTH: f32 = 520.;
const DENY_LABEL: &str = "Deny";
const ALLOW_IN_SESSION_LABEL: &str = "Allow this command in this session";
const APPROVE_LABEL: &str = "Approve";

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    // Nothing is bound to Enter (P4 of the O2 plan): the destructive action — letting a command
    // run as root on a server — must be clicked deliberately, never hit by a stray keypress.
    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        AgentApprovalAction::Close,
        id!(AgentApprovalDialog::ui_name()),
    )]);
}

/// Builds the dialog's title and body for `request`. A pure function so its text can be tested
/// without a view. Shows the command or write content verbatim (P10 of the O2 plan): the person
/// deciding has to see exactly what will run.
pub(crate) fn content(request: &ApprovalRequest) -> (String, String) {
    let title = match &request.subject {
        ApprovalSubject::Command { .. } => format!("Run on {}?", request.session_label),
        ApprovalSubject::Write { .. } => format!("Write a file on {}?", request.session_label),
        ApprovalSubject::Pairing { .. } => "Pair an agent with Warp?".to_owned(),
    };

    let mut lines = Vec::new();
    match &request.subject {
        ApprovalSubject::Command {
            command,
            cwd,
            visible,
        } => {
            lines.push(agent_line(&request.agent));
            lines.push(
                if *visible {
                    "Runs visibly in the terminal"
                } else {
                    "Runs in the background"
                }
                .to_owned(),
            );
            if let Some(cwd) = cwd {
                lines.push(format!("Directory: {}", printable(cwd)));
            }
            lines.push(printable(command));
        }
        ApprovalSubject::Write {
            path,
            bytes,
            creates,
            preview,
            preview_truncated_lines,
        } => {
            lines.push(agent_line(&request.agent));
            lines.push(format!("Path: {}", printable(path)));
            lines.push(format!("Size: {bytes} bytes"));
            lines.push(
                if *creates {
                    "Creates a new file"
                } else {
                    "Replaces the existing file (a backup is kept on the server)"
                }
                .to_owned(),
            );
            let mut preview_block = printable(preview);
            if *preview_truncated_lines > 0 {
                preview_block.push_str(&format!("\n… {preview_truncated_lines} more lines"));
            }
            lines.push(preview_block);
        }
        ApprovalSubject::Pairing { name } => {
            lines.push(format!("Agent: {}", printable(name)));
            lines.push(
                "Pairing lets Warp show which agent sends each request. It does not stop \
                 other programs running as your user."
                    .to_owned(),
            );
        }
    }
    lines.push(format!(
        "Denied automatically at {} if no one answers.",
        format_deadline(request.deadline)
    ));
    (title, lines.join("\n\n"))
}

/// "Agent: claude-code (paired)" once pairing resolved a verified `agent_id` — that name is the
/// paired identity, not necessarily what the request's own `agent` field claims, since the two can
/// differ once the id has been disambiguated with a "-2" suffix. Otherwise "Agent: claude-code
/// (unverified name)" for a self-reported name, or a name-less fallback for neither.
fn agent_line(agent: &AgentLabel) -> String {
    if let Some(agent_id) = &agent.agent_id {
        return format!("Agent: {} (paired)", printable(agent_id));
    }
    match &agent.claimed {
        Some(name) => format!("Agent: {} (unverified name)", printable(name)),
        None => "Agent: an agent that did not give its name".to_owned(),
    }
}

fn format_deadline(deadline: SystemTime) -> String {
    DateTime::<Local>::from(deadline)
        .format("%H:%M")
        .to_string()
}

pub struct AgentApprovalDialog {
    request_id: Option<Uuid>,
    deny_button: ViewHandle<ActionButton>,
    allow_in_session_button: ViewHandle<ActionButton>,
    approve_button: ViewHandle<ActionButton>,
}

impl AgentApprovalDialog {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let deny_button = ctx.add_typed_action_view(|_| {
            ActionButton::new(DENY_LABEL, NakedTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(AgentApprovalAction::Decide(ApprovalDecision::Deny));
            })
        });
        let allow_in_session_button = ctx.add_typed_action_view(|_| {
            ActionButton::new(ALLOW_IN_SESSION_LABEL, SecondaryTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(AgentApprovalAction::Decide(
                    ApprovalDecision::AllowInSession,
                ));
            })
        });
        let approve_button = ctx.add_typed_action_view(|_| {
            ActionButton::new(APPROVE_LABEL, DangerPrimaryTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(AgentApprovalAction::Decide(ApprovalDecision::Approve));
            })
        });
        Self {
            request_id: None,
            deny_button,
            allow_in_session_button,
            approve_button,
        }
    }

    /// Shows `request_id`. The dialog reads the request itself from [`AgentBridgeModel`] at
    /// render time, so it always reflects the live queue.
    pub fn set_request(&mut self, request_id: Uuid, ctx: &mut ViewContext<Self>) {
        self.request_id = Some(request_id);
        ctx.notify();
    }

    /// The request currently shown, if any — so Workspace can tell whether it is still in
    /// [`AgentBridgeModel`]'s queue after an `ApprovalsChanged` event.
    pub fn request_id(&self) -> Option<Uuid> {
        self.request_id
    }
}

impl Entity for AgentApprovalDialog {
    type Event = AgentApprovalEvent;
}

impl View for AgentApprovalDialog {
    fn ui_name() -> &'static str {
        "AgentApprovalDialog"
    }

    fn on_focus(&mut self, _focus_ctx: &warpui::FocusContext, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let request = self
            .request_id
            .and_then(|id| AgentBridgeModel::as_ref(app).approval(id));
        // The request can disappear between the toast that opened this dialog and Workspace
        // acting on `ApprovalsChanged` to close it (mục 3.7 of the O2 plan) — that one frame
        // renders empty chrome, the same fallback `WarpSyncConfirmDialog` uses while it has no
        // request either, rather than an unproven zero-child layout.
        let (title, body, shows_allow_in_session) = match request {
            Some(request) => {
                let (title, body) = content(request);
                let shows_allow_in_session =
                    matches!(request.subject, ApprovalSubject::Command { .. });
                (title, body, shows_allow_in_session)
            }
            None => (String::new(), String::new(), false),
        };

        let appearance = Appearance::as_ref(app);
        let deny_button = Container::new(ChildView::new(&self.deny_button).finish())
            .with_margin_right(12.)
            .finish();

        let mut dialog = Dialog::new(
            title,
            Some(body),
            UiComponentStyles {
                width: Some(DIALOG_WIDTH),
                ..dialog_styles(appearance)
            },
        )
        .with_bottom_row_child(deny_button);
        if shows_allow_in_session {
            dialog = dialog.with_bottom_row_child(
                Container::new(ChildView::new(&self.allow_in_session_button).finish())
                    .with_margin_right(12.)
                    .finish(),
            );
        }
        let dialog = dialog
            .with_bottom_row_child(ChildView::new(&self.approve_button).finish())
            .build()
            .finish();

        let mut stack = Stack::new();
        stack.add_positioned_child(
            dialog,
            OffsetPositioning::offset_from_parent(
                vec2f(0., 0.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            ),
        );

        Container::new(Align::new(stack.finish()).finish())
            .with_background_color(Fill::blur().into())
            .with_corner_radius(app.windows().window_corner_radius())
            .finish()
    }
}

pub enum AgentApprovalEvent {
    Decided {
        request_id: Uuid,
        decision: ApprovalDecision,
    },
    Closed,
}

#[derive(Debug)]
pub enum AgentApprovalAction {
    Decide(ApprovalDecision),
    Close,
}

impl TypedActionView for AgentApprovalDialog {
    type Action = AgentApprovalAction;

    fn handle_action(&mut self, action: &AgentApprovalAction, ctx: &mut ViewContext<Self>) {
        match action {
            // The request stays queued: closing the dialog is not a decision (mục 3.7's keymap
            // note — Esc closes, it does not deny).
            AgentApprovalAction::Close => ctx.emit(AgentApprovalEvent::Closed),
            AgentApprovalAction::Decide(decision) => {
                let Some(request_id) = self.request_id else {
                    return;
                };
                ctx.emit(AgentApprovalEvent::Decided {
                    request_id,
                    decision: *decision,
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "approval_dialog_tests.rs"]
mod tests;
