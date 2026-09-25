//! Settings UI for Warp Sync: where mirrors are kept and how much one transfer may move.
use std::cell::RefCell;
use std::collections::HashMap;

use settings::Setting as _;
use warp_errors::report_if_error;
use warpui::elements::{Element, MouseStateHandle};
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle};

use super::settings_page::{
    LocalOnlyIconState, MatchData, PageTitle, PageType, SettingsPageEvent, SettingsPageMeta,
    SettingsPageViewHandle, SettingsWidget, render_body_item,
};
use super::{SettingsSection, ToggleState};
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, InteractionState, SingleLineEditorOptions, TextOptions,
};
use crate::features::FeatureFlag;
use crate::settings::{
    WarpSyncMaxDownloadMib, WarpSyncMaxUploadMib, WarpSyncMirrorRoot, WarpSyncSettings,
};
use crate::warp_sync::config::resolve_mirror_root;
use crate::warp_sync::{MAX_CONFIGURABLE_DOWNLOAD_MIB, MAX_CONFIGURABLE_UPLOAD_MIB};

const MIRROR_ROOT_INPUT_WIDTH: f32 = 320.;
const LIMIT_INPUT_WIDTH: f32 = 80.;
const INPUT_VERTICAL_PADDING: f32 = 7.;
const INPUT_HORIZONTAL_PADDING: f32 = 12.;
const MIN_LIMIT_MIB: u32 = 1;

pub struct WarpSyncSettingsPageView {
    page: PageType<Self>,
    local_only_icon_tooltip_states: RefCell<HashMap<String, MouseStateHandle>>,
    mirror_root_editor: ViewHandle<EditorView>,
    max_download_editor: ViewHandle<EditorView>,
    max_upload_editor: ViewHandle<EditorView>,
}

impl WarpSyncSettingsPageView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let mirror_root_editor = Self::editor(Self::commit_mirror_root, ctx);
        let max_download_editor = Self::editor(Self::commit_max_download, ctx);
        let max_upload_editor = Self::editor(Self::commit_max_upload, ctx);
        mirror_root_editor.update(ctx, |editor, ctx| {
            editor.set_placeholder_text("~/.warp/mirrors", ctx)
        });

        ctx.subscribe_to_model(&WarpSyncSettings::handle(ctx), |view, _, _, ctx| {
            view.show_stored_values(ctx);
            ctx.notify();
        });

        let widgets: Vec<Box<dyn SettingsWidget<View = Self>>> = vec![
            Box::new(MirrorRootWidget),
            Box::new(MaxDownloadWidget),
            Box::new(MaxUploadWidget),
        ];
        let view = Self {
            page: PageType::new_uncategorized(widgets, Some(PageTitle::new("Warp Sync"))),
            local_only_icon_tooltip_states: RefCell::new(HashMap::new()),
            mirror_root_editor,
            max_download_editor,
            max_upload_editor,
        };
        view.show_stored_values(ctx);
        view
    }

    /// An editor that commits its text when the user presses Enter or leaves the field.
    fn editor(
        commit: fn(&mut Self, &mut ViewContext<Self>),
        ctx: &mut ViewContext<Self>,
    ) -> ViewHandle<EditorView> {
        let editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions {
                        font_size_override: Some(appearance.ui_font_size()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ctx,
            )
        });
        ctx.subscribe_to_view(&editor, move |me, _, event, ctx| match event {
            EditorEvent::Blurred | EditorEvent::Enter => commit(me, ctx),
            EditorEvent::Escape => ctx.emit(SettingsPageEvent::FocusModal),
            _ => {}
        });
        editor
    }

    /// Puts the stored values into the editors, leaving an editor alone if it already shows its
    /// value so that the caret does not jump.
    fn show_stored_values(&self, ctx: &mut ViewContext<Self>) {
        let (mirror_root, max_download, max_upload) = {
            let settings = WarpSyncSettings::as_ref(ctx);
            (
                settings.mirror_root.value().clone(),
                settings.max_download_mib.value().to_string(),
                settings.max_upload_mib.value().to_string(),
            )
        };
        for (editor, text) in [
            (&self.mirror_root_editor, mirror_root),
            (&self.max_download_editor, max_download),
            (&self.max_upload_editor, max_upload),
        ] {
            if editor.as_ref(ctx).buffer_text(ctx) != text {
                editor.update(ctx, |editor, ctx| {
                    editor.set_interaction_state(InteractionState::Editable, ctx);
                    editor.set_buffer_text(&text, ctx);
                });
            }
        }
    }

    fn commit_mirror_root(&mut self, ctx: &mut ViewContext<Self>) {
        let input = self.mirror_root_editor.as_ref(ctx).buffer_text(ctx);
        let input = input.trim();
        if resolve_mirror_root(input, dirs::home_dir().as_deref()).is_ok() {
            WarpSyncSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.mirror_root.set_value(input.to_owned(), ctx));
            });
        }
        self.show_stored_values(ctx);
        ctx.notify();
    }

    fn commit_max_download(&mut self, ctx: &mut ViewContext<Self>) {
        let input = self.max_download_editor.as_ref(ctx).buffer_text(ctx);
        if let Some(mib) = parse_limit(&input, MAX_CONFIGURABLE_DOWNLOAD_MIB) {
            WarpSyncSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.max_download_mib.set_value(mib, ctx));
            });
        }
        self.show_stored_values(ctx);
        ctx.notify();
    }

    fn commit_max_upload(&mut self, ctx: &mut ViewContext<Self>) {
        let input = self.max_upload_editor.as_ref(ctx).buffer_text(ctx);
        if let Some(mib) = parse_limit(&input, MAX_CONFIGURABLE_UPLOAD_MIB) {
            WarpSyncSettings::handle(ctx).update(ctx, |settings, ctx| {
                report_if_error!(settings.max_upload_mib.set_value(mib, ctx));
            });
        }
        self.show_stored_values(ctx);
        ctx.notify();
    }
}

/// A limit in MiB, or `None` if `input` is not a whole number within `MIN_LIMIT_MIB..=max`.
fn parse_limit(input: &str, max: u32) -> Option<u32> {
    input
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|mib| (MIN_LIMIT_MIB..=max).contains(mib))
}

impl Entity for WarpSyncSettingsPageView {
    type Event = SettingsPageEvent;
}

impl TypedActionView for WarpSyncSettingsPageView {
    type Action = ();

    fn handle_action(&mut self, _action: &(), _ctx: &mut ViewContext<Self>) {}
}

impl View for WarpSyncSettingsPageView {
    fn ui_name() -> &'static str {
        "WarpSyncSettingsPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

impl SettingsPageMeta for WarpSyncSettingsPageView {
    fn section() -> SettingsSection {
        SettingsSection::WarpSync
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        cfg!(not(target_family = "wasm")) && FeatureFlag::WarpSync.is_enabled()
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<WarpSyncSettingsPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<WarpSyncSettingsPageView>) -> Self {
        SettingsPageViewHandle::WarpSync(view_handle)
    }
}

fn text_input(
    editor: &ViewHandle<EditorView>,
    width: f32,
    appearance: &Appearance,
) -> Box<dyn Element> {
    appearance
        .ui_builder()
        .text_input(editor.clone())
        .with_style(UiComponentStyles {
            width: Some(width),
            padding: Some(Coords {
                top: INPUT_VERTICAL_PADDING,
                bottom: INPUT_VERTICAL_PADDING,
                left: INPUT_HORIZONTAL_PADDING,
                right: INPUT_HORIZONTAL_PADDING,
            }),
            background: Some(appearance.theme().surface_2().into()),
            ..Default::default()
        })
        .build()
        .finish()
}

struct MirrorRootWidget;

impl SettingsWidget for MirrorRootWidget {
    type View = WarpSyncSettingsPageView;

    fn search_terms(&self) -> &str {
        "warp sync mirror folder directory path local remote files download"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        render_body_item::<()>(
            "Mirror folder".into(),
            None,
            LocalOnlyIconState::for_setting(
                WarpSyncMirrorRoot::storage_key(),
                WarpSyncMirrorRoot::sync_to_cloud(),
                &mut view.local_only_icon_tooltip_states.borrow_mut(),
                app,
            ),
            ToggleState::Enabled,
            appearance,
            text_input(&view.mirror_root_editor, MIRROR_ROOT_INPUT_WIDTH, appearance),
            Some(
                "Where files downloaded from remote hosts are kept. Leave empty for \
                 ~/.warp/mirrors. Existing mirrors are not moved."
                    .to_owned(),
            ),
        )
    }
}

struct MaxDownloadWidget;

impl SettingsWidget for MaxDownloadWidget {
    type View = WarpSyncSettingsPageView;

    fn search_terms(&self) -> &str {
        "warp sync download limit maximum size mib megabytes"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        render_body_item::<()>(
            "Download limit (MiB)".into(),
            None,
            LocalOnlyIconState::for_setting(
                WarpSyncMaxDownloadMib::storage_key(),
                WarpSyncMaxDownloadMib::sync_to_cloud(),
                &mut view.local_only_icon_tooltip_states.borrow_mut(),
                app,
            ),
            ToggleState::Enabled,
            appearance,
            text_input(&view.max_download_editor, LIMIT_INPUT_WIDTH, appearance),
            Some(format!(
                "The most data, measured on the remote host, that one download may transfer \
                 (1 to {MAX_CONFIGURABLE_DOWNLOAD_MIB}). Large downloads are slow because \
                 they pass through the terminal."
            )),
        )
    }
}

struct MaxUploadWidget;

impl SettingsWidget for MaxUploadWidget {
    type View = WarpSyncSettingsPageView;

    fn search_terms(&self) -> &str {
        "warp sync upload limit maximum size mib megabytes"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        render_body_item::<()>(
            "Upload limit (MiB)".into(),
            None,
            LocalOnlyIconState::for_setting(
                WarpSyncMaxUploadMib::storage_key(),
                WarpSyncMaxUploadMib::sync_to_cloud(),
                &mut view.local_only_icon_tooltip_states.borrow_mut(),
                app,
            ),
            ToggleState::Enabled,
            appearance,
            text_input(&view.max_upload_editor, LIMIT_INPUT_WIDTH, appearance),
            Some(format!(
                "The largest compressed size that one upload may have (1 to \
                 {MAX_CONFIGURABLE_UPLOAD_MIB}). Uploads are typed into the remote shell, so \
                 large ones are slow."
            )),
        )
    }
}

#[cfg(test)]
#[path = "warp_sync_page_tests.rs"]
mod tests;
