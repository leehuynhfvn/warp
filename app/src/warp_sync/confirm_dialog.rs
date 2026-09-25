use std::borrow::Cow;
use std::path::PathBuf;

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

use super::editor::{EditorCli, EditorRequest};
use super::model::{CompareSummary, PendingId, UploadSummary, format_size, pluralize_count};
use super::paths::printable;
use super::remote_check::{RemoteCheck, RemoteConflicts};
use crate::appearance::Appearance;
use crate::ui_components::dialog::{Dialog, dialog_styles};
use crate::view_components::action_button::{
    ActionButton, DangerPrimaryTheme, NakedTheme, PrimaryTheme,
};

const DIALOG_WIDTH: f32 = 520.;
const MAX_LISTED_PATHS: usize = 10;
const CANCEL_LABEL: &str = "Cancel";

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

/// What confirming the dialog does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmKind {
    /// Resumes a download that would discard local edits.
    OverwriteLocalChanges { id: PendingId },
    /// Resumes an upload.
    Upload { id: PendingId },
    /// Opens the comparison: side by side in the external editor when there is one, otherwise
    /// the written comparison in Warp.
    CompareResult {
        diff_path: PathBuf,
        editor_request: Option<EditorRequest>,
    },
}

impl ConfirmKind {
    /// The pending operation that cancelling the dialog abandons, if any.
    pub fn pending_id(&self) -> Option<PendingId> {
        match self {
            Self::OverwriteLocalChanges { id } | Self::Upload { id } => Some(*id),
            Self::CompareResult { .. } => None,
        }
    }
}

/// Whether confirming is an action that can lose data, which decides how its button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmStyle {
    Destructive,
    Neutral,
}

/// What the dialog asks and what confirming it does.
#[derive(Debug, Clone)]
pub struct ConfirmRequest {
    pub kind: ConfirmKind,
    title: String,
    body: String,
    confirm_label: Cow<'static, str>,
    cancel_label: &'static str,
    style: ConfirmStyle,
}

impl ConfirmRequest {
    pub fn overwrite_local_changes(id: PendingId, modified_files: &[String]) -> Self {
        let body = format!(
            "Downloading replaces your local copy. These files differ from what was last \
             downloaded, and their changes will be lost:\n\n{}",
            bullet_list(modified_files)
        );
        Self {
            kind: ConfirmKind::OverwriteLocalChanges { id },
            title: "Overwrite local changes?".to_owned(),
            body,
            confirm_label: "Overwrite".into(),
            cancel_label: CANCEL_LABEL,
            style: ConfirmStyle::Destructive,
        }
    }

    pub fn upload(id: PendingId, summary: &UploadSummary) -> Self {
        Self {
            kind: ConfirmKind::Upload { id },
            title: format!(
                "Upload to {}@{}?",
                printable(&summary.remote_user),
                printable(&summary.hostname)
            ),
            body: upload_body(summary),
            confirm_label: "Upload".into(),
            cancel_label: CANCEL_LABEL,
            style: ConfirmStyle::Destructive,
        }
    }

    pub fn compare_result(summary: &CompareSummary, editor: Option<EditorCli>) -> Self {
        let confirm_label = match editor {
            Some(editor) => format!("Open in {}", editor.name()).into(),
            None => "Open diff".into(),
        };
        Self {
            kind: ConfirmKind::CompareResult {
                diff_path: summary.diff_path.clone(),
                editor_request: editor.map(|_| summary.editor_request()),
            },
            title: format!(
                "{} with {}@{}",
                pluralize_count(summary.differences.len(), "difference"),
                summary.remote_user,
                summary.hostname
            ),
            body: compare_body(summary, editor.is_some()),
            confirm_label,
            cancel_label: "Close",
            style: ConfirmStyle::Neutral,
        }
    }
}

fn compare_body(summary: &CompareSummary, opens_in_editor: bool) -> String {
    let changes: Vec<String> = summary
        .differences
        .iter()
        .map(|difference| format!("{}: {}", difference.change.label(), difference.remote_path))
        .collect();
    [
        format!("{}:{}", summary.hostname, printable(&summary.remote_path)),
        format!(
            "{} differ, {} identical.",
            pluralize_count(summary.differences.len(), "file"),
            summary.identical_files
        ),
        bullet_list(&changes),
        if opens_in_editor {
            "Side by side, the server is on the left and your local mirror on the right. \
             Uploading makes the server match the local mirror."
        } else {
            "In the diff, '-' is the server and '+' is your local mirror. Uploading makes the \
             server match the local mirror."
        }
        .to_owned(),
    ]
    .join("\n\n")
}

fn upload_body(summary: &UploadSummary) -> String {
    let server = match &summary.server_id_tail {
        Some(tail) => format!(
            "{} (machine id …{})",
            printable(&summary.hostname),
            printable(tail)
        ),
        None => printable(&summary.hostname),
    };
    let mut sections = vec![
        format!("{server}:{}", printable(&summary.remote_path)),
        format!(
            "{} and {}, {}",
            pluralize_count(summary.files, "file"),
            pluralize_count(summary.dirs, "folder"),
            format_size(summary.content_bytes)
        ),
    ];
    match &summary.creates_under {
        Some(anchor) => sections.push(format!(
            "Creates on the server, inside {}:\n{}",
            printable(anchor),
            bullet_list(&new_entry_lines(summary))
        )),
        None => {
            sections.extend(remote_check_sections(&summary.remote_check));
            if !summary.new_files.is_empty() {
                sections.push(format!(
                    "New on the server:\n{}",
                    bullet_list(&new_entry_lines(summary))
                ));
            }
        }
    }
    if !summary.missing_locally.is_empty() {
        sections.push(format!(
            "Missing from the local mirror (they will NOT be deleted on the server):\n{}",
            bullet_list(&summary.missing_locally)
        ));
    }
    sections.push(
        match summary.creates_under {
            Some(_) => "Nothing on the server is replaced, so no backup is made.",
            None => "Whatever is replaced is first saved under ~/.warp-sync/backups on the server.",
        }
        .to_owned(),
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

/// The new entries, each with the mode it gets on the server where that is known.
fn new_entry_lines(summary: &UploadSummary) -> Vec<String> {
    summary
        .new_files
        .iter()
        .map(|path| match summary.new_modes.get(path) {
            Some(mode) => format!("{path} (mode {mode:04o})"),
            None => path.clone(),
        })
        .collect()
}

fn remote_check_sections(remote_check: &RemoteCheck) -> Vec<String> {
    match remote_check {
        RemoteCheck::Unavailable => vec![
            "Could not check whether the server's files changed since the last sync (the server \
             needs sha256sum or shasum)."
                .to_owned(),
        ],
        RemoteCheck::Checked(conflicts) if conflicts.is_empty() => {
            vec!["The server's files are unchanged since the last sync.".to_owned()]
        }
        RemoteCheck::Checked(conflicts) => conflict_sections(conflicts),
    }
}

fn conflict_sections(conflicts: &RemoteConflicts) -> Vec<String> {
    [
        (
            "Warning: changed on the server since the last sync (uploading OVERWRITES these \
             changes):",
            &conflicts.changed,
        ),
        (
            "Warning: no longer on the server, or unreadable (uploading recreates them):",
            &conflicts.missing,
        ),
        (
            "Warning: new here, but already on the server (uploading replaces them):",
            &conflicts.already_exist,
        ),
    ]
    .into_iter()
    .filter(|(_, paths)| !paths.is_empty())
    .map(|(heading, paths)| format!("{heading}\n{}", bullet_list(paths)))
    .collect()
}

fn bullet_list(paths: &[String]) -> String {
    let mut lines: Vec<String> = paths
        .iter()
        .take(MAX_LISTED_PATHS)
        .map(|path| format!("• {}", printable(path)))
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
            ActionButton::new(CANCEL_LABEL, NakedTheme).on_click(|ctx| {
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
            button.set_label(request.confirm_label.clone(), ctx);
            match request.style {
                ConfirmStyle::Destructive => button.set_theme(DangerPrimaryTheme, ctx),
                ConfirmStyle::Neutral => button.set_theme(PrimaryTheme, ctx),
            }
        });
        self.cancel_button.update(ctx, |button, ctx| {
            button.set_label(request.cancel_label, ctx)
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
