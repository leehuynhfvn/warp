use pathfinder_geometry::vector::vec2f;
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

use super::model::{PendingId, UploadSummary, format_size, pluralize_count};
use crate::appearance::Appearance;
use crate::ui_components::dialog::{Dialog, dialog_styles};
use crate::view_components::action_button::{ActionButton, DangerPrimaryTheme, NakedTheme};

const DIALOG_WIDTH: f32 = 520.;
const MAX_LISTED_PATHS: usize = 10;

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    // Nothing is bound to Enter: both dialogs guard against data loss, so the destructive button
    // must be clicked deliberately.
    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        WarpSyncConfirmAction::Cancel,
        id!(WarpSyncConfirmDialog::ui_name()),
    )]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmKind {
    OverwriteLocalChanges,
    Upload,
}

/// What the dialog asks and which pending operation a confirmation resumes.
#[derive(Debug, Clone)]
pub struct ConfirmRequest {
    pub id: PendingId,
    pub kind: ConfirmKind,
    title: String,
    body: String,
    confirm_label: &'static str,
}

impl ConfirmRequest {
    pub fn overwrite_local_changes(id: PendingId, modified_files: &[String]) -> Self {
        let body = format!(
            "Downloading replaces your local copy. These files differ from what was last \
             downloaded, and their changes will be lost:\n\n{}",
            bullet_list(modified_files)
        );
        Self {
            id,
            kind: ConfirmKind::OverwriteLocalChanges,
            title: "Overwrite local changes?".to_owned(),
            body,
            confirm_label: "Overwrite",
        }
    }

    pub fn upload(id: PendingId, summary: &UploadSummary) -> Self {
        Self {
            id,
            kind: ConfirmKind::Upload,
            title: format!("Upload to {}@{}?", summary.remote_user, summary.hostname),
            body: upload_body(summary),
            confirm_label: "Upload",
        }
    }
}

fn upload_body(summary: &UploadSummary) -> String {
    let server = match &summary.server_id_tail {
        Some(tail) => format!("{} (machine id …{tail})", summary.hostname),
        None => summary.hostname.clone(),
    };
    let mut sections = vec![
        format!("{server}:{}", summary.remote_path),
        format!(
            "{} and {}, {}",
            pluralize_count(summary.files, "file"),
            pluralize_count(summary.dirs, "folder"),
            format_size(summary.content_bytes)
        ),
    ];
    if !summary.new_files.is_empty() {
        sections.push(format!(
            "New on the server:\n{}",
            bullet_list(&summary.new_files)
        ));
    }
    if !summary.missing_locally.is_empty() {
        sections.push(format!(
            "Missing from the local mirror (they will NOT be deleted on the server):\n{}",
            bullet_list(&summary.missing_locally)
        ));
    }
    sections.push(
        "Whatever is replaced is first saved under ~/.warp-sync/backups on the server.".to_owned(),
    );
    if summary.ownership_may_be_incomplete {
        sections.push(
            "Warning: the server's tar is not GNU tar, so file ownership may not be restored \
             completely."
                .to_owned(),
        );
    }
    sections.join("\n\n")
}

fn bullet_list(paths: &[String]) -> String {
    let mut lines: Vec<String> = paths
        .iter()
        .take(MAX_LISTED_PATHS)
        .map(|path| format!("• {path}"))
        .collect();
    if paths.len() > MAX_LISTED_PATHS {
        lines.push(format!("… and {} more", paths.len() - MAX_LISTED_PATHS));
    }
    lines.join("\n")
}

pub struct WarpSyncConfirmDialog {
    cancel_button: ViewHandle<ActionButton>,
    confirm_button: ViewHandle<ActionButton>,
    request: Option<ConfirmRequest>,
}

impl WarpSyncConfirmDialog {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let cancel_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Cancel", NakedTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(WarpSyncConfirmAction::Cancel);
            })
        });
        let confirm_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Confirm", DangerPrimaryTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(WarpSyncConfirmAction::Confirm);
            })
        });
        Self {
            cancel_button,
            confirm_button,
            request: None,
        }
    }

    /// Shows `request`, returning the request it replaced if the dialog was already open.
    pub fn set_request(
        &mut self,
        request: ConfirmRequest,
        ctx: &mut ViewContext<Self>,
    ) -> Option<ConfirmRequest> {
        self.confirm_button.update(ctx, |button, ctx| {
            button.set_label(request.confirm_label, ctx)
        });
        let replaced = self.request.replace(request);
        ctx.notify();
        replaced
    }
}

impl Entity for WarpSyncConfirmDialog {
    type Event = WarpSyncConfirmEvent;
}

impl View for WarpSyncConfirmDialog {
    fn ui_name() -> &'static str {
        "WarpSyncConfirmDialog"
    }

    fn on_focus(&mut self, _focus_ctx: &warpui::FocusContext, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let (title, body) = self
            .request
            .as_ref()
            .map(|request| (request.title.clone(), request.body.clone()))
            .unwrap_or_default();

        let cancel_button = Container::new(ChildView::new(&self.cancel_button).finish())
            .with_margin_right(12.)
            .finish();
        let dialog = Dialog::new(
            title,
            Some(body),
            UiComponentStyles {
                width: Some(DIALOG_WIDTH),
                ..dialog_styles(appearance)
            },
        )
        .with_bottom_row_child(cancel_button)
        .with_bottom_row_child(ChildView::new(&self.confirm_button).finish())
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

pub enum WarpSyncConfirmEvent {
    Confirm { request: ConfirmRequest },
    Cancel { request: ConfirmRequest },
}

#[derive(Debug)]
pub enum WarpSyncConfirmAction {
    Confirm,
    Cancel,
}

impl TypedActionView for WarpSyncConfirmDialog {
    type Action = WarpSyncConfirmAction;

    fn handle_action(&mut self, action: &WarpSyncConfirmAction, ctx: &mut ViewContext<Self>) {
        let Some(request) = self.request.take() else {
            return;
        };
        match action {
            WarpSyncConfirmAction::Confirm => ctx.emit(WarpSyncConfirmEvent::Confirm { request }),
            WarpSyncConfirmAction::Cancel => ctx.emit(WarpSyncConfirmEvent::Cancel { request }),
        }
    }
}

#[cfg(test)]
#[path = "confirm_dialog_tests.rs"]
mod tests;
