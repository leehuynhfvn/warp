use pathfinder_geometry::vector::vec2f;
use warp_core::ui::theme::Fill;
use warpui::elements::{
    Align, Border, ChildAnchor, ChildView, Container, CornerRadius, Flex, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, Radius, Stack, Text,
};
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, InteractionState, SingleLineEditorOptions, TextOptions,
};
use crate::ui_components::dialog::{Dialog, dialog_styles};
use crate::view_components::action_button::{ActionButton, NakedTheme, PrimaryTheme};

const DIALOG_WIDTH: f32 = 520.;
const EDITOR_FONT_SIZE: f32 = 13.;
const EDITOR_PADDING: f32 = 8.;
const EDITOR_BORDER_WIDTH: f32 = 1.;
const EDITOR_BORDER_RADIUS: f32 = 4.;
const ERROR_FONT_SIZE: f32 = 12.;
const ERROR_MARGIN_TOP: f32 = 8.;
const PATH_PLACEHOLDER: &str = "/etc/nginx/nginx.conf";

/// What the entered path is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathPromptKind {
    Download,
    Upload,
    Compare,
}

impl PathPromptKind {
    fn title(self) -> &'static str {
        match self {
            Self::Download => "Download from the server",
            Self::Upload => "Upload to the server",
            Self::Compare => "Compare with the server",
        }
    }

    fn confirm_label(self) -> &'static str {
        match self {
            Self::Download => "Download",
            Self::Upload => "Upload",
            Self::Compare => "Compare",
        }
    }
}

pub struct WarpSyncPathPrompt {
    editor: ViewHandle<EditorView>,
    cancel_button: ViewHandle<ActionButton>,
    confirm_button: ViewHandle<ActionButton>,
    kind: PathPromptKind,
    hint: String,
    error: Option<String>,
}

impl WarpSyncPathPrompt {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let mut editor = EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions::ui_text(Some(EDITOR_FONT_SIZE), appearance),
                    soft_wrap: false,
                    ..Default::default()
                },
                ctx,
            );
            editor.set_placeholder_text(PATH_PLACEHOLDER, ctx);
            editor
        });
        ctx.subscribe_to_view(&editor, |me, _, event, ctx| {
            me.handle_editor_event(event, ctx);
        });

        let cancel_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Cancel", NakedTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(WarpSyncPathPromptAction::Cancel);
            })
        });
        let confirm_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Download", PrimaryTheme).on_click(|ctx| {
                ctx.dispatch_typed_action(WarpSyncPathPromptAction::Submit);
            })
        });
        Self {
            editor,
            cancel_button,
            confirm_button,
            kind: PathPromptKind::Download,
            hint: String::new(),
            error: None,
        }
    }

    /// Clears the input and shows the prompt for `kind`. `hint` tells the user what a relative
    /// path is relative to.
    pub fn open(&mut self, kind: PathPromptKind, hint: String, ctx: &mut ViewContext<Self>) {
        self.kind = kind;
        self.hint = hint;
        self.error = None;
        self.confirm_button.update(ctx, |button, ctx| {
            button.set_label(kind.confirm_label(), ctx)
        });
        self.editor.update(ctx, |editor, ctx| {
            editor.set_interaction_state(InteractionState::Editable, ctx);
            editor.clear_buffer(ctx);
        });
        ctx.notify();
    }

    pub fn set_error(&mut self, message: String, ctx: &mut ViewContext<Self>) {
        self.error = Some(message);
        ctx.notify();
    }

    fn submit(&mut self, ctx: &mut ViewContext<Self>) {
        let input = self.editor.as_ref(ctx).buffer_text(ctx);
        if input.trim().is_empty() {
            self.set_error("Enter a path".to_owned(), ctx);
            return;
        }
        ctx.emit(WarpSyncPathPromptEvent::Submit {
            kind: self.kind,
            input,
        });
    }

    fn handle_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Enter => self.submit(ctx),
            EditorEvent::Escape => ctx.emit(WarpSyncPathPromptEvent::Cancel),
            EditorEvent::Edited(_) => {
                if self.error.take().is_some() {
                    ctx.notify();
                }
            }
            _ => {}
        }
    }
}

impl Entity for WarpSyncPathPrompt {
    type Event = WarpSyncPathPromptEvent;
}

impl View for WarpSyncPathPrompt {
    fn ui_name() -> &'static str {
        "WarpSyncPathPrompt"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.editor);
            ctx.notify();
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let input = Container::new(ChildView::new(&self.editor).finish())
            .with_uniform_padding(EDITOR_PADDING)
            .with_background(theme.surface_2())
            .with_border(Border::all(EDITOR_BORDER_WIDTH).with_border_fill(theme.surface_3()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(EDITOR_BORDER_RADIUS)))
            .finish();
        let mut content = Flex::column().with_child(input);
        if let Some(error) = &self.error {
            let error_text =
                Text::new_inline(error.clone(), appearance.ui_font_family(), ERROR_FONT_SIZE)
                    .with_color(theme.ui_error_color())
                    .finish();
            content.add_child(
                Container::new(error_text)
                    .with_margin_top(ERROR_MARGIN_TOP)
                    .finish(),
            );
        }

        let cancel_button = Container::new(ChildView::new(&self.cancel_button).finish())
            .with_margin_right(12.)
            .finish();
        let dialog = Dialog::new(
            self.kind.title().to_owned(),
            Some(self.hint.clone()),
            UiComponentStyles {
                width: Some(DIALOG_WIDTH),
                ..dialog_styles(appearance)
            },
        )
        .with_child(content.finish())
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

pub enum WarpSyncPathPromptEvent {
    Submit { kind: PathPromptKind, input: String },
    Cancel,
}

#[derive(Debug)]
pub enum WarpSyncPathPromptAction {
    Submit,
    Cancel,
}

impl TypedActionView for WarpSyncPathPrompt {
    type Action = WarpSyncPathPromptAction;

    fn handle_action(&mut self, action: &WarpSyncPathPromptAction, ctx: &mut ViewContext<Self>) {
        match action {
            WarpSyncPathPromptAction::Submit => self.submit(ctx),
            WarpSyncPathPromptAction::Cancel => ctx.emit(WarpSyncPathPromptEvent::Cancel),
        }
    }
}
