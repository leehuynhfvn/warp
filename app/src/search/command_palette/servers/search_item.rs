use fuzzy_match::FuzzyMatchResult;
use ordered_float::OrderedFloat;
use warpui::elements::{
    Align, ConstrainedBox, Container, Flex, Highlight, ParentElement, Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::{AppContext, Element, SingletonEntity};

use crate::appearance::Appearance;
use crate::host_directory::Host;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::render_util::render_search_item_icon;
use crate::search::result_renderer::ItemHighlightState;
use crate::ui_components::icons::Icon;

const ROW_HEIGHT: f32 = 40.;
const TAGS_MARGIN_RIGHT: f32 = 14.;

/// SearchItem for a matching server.
#[derive(Debug)]
pub struct SearchItem {
    alias: String,
    tags: Vec<String>,
    match_result: FuzzyMatchResult,
}

impl SearchItem {
    pub fn new(host: &Host, match_result: FuzzyMatchResult) -> Self {
        Self {
            alias: host.alias.clone(),
            tags: host.tags.clone(),
            match_result,
        }
    }

    fn connect(&self) -> CommandPaletteItemAction {
        CommandPaletteItemAction::ConnectToServer {
            alias: self.alias.clone(),
        }
    }
}

impl crate::search::item::SearchItem for SearchItem {
    type Action = CommandPaletteItemAction;

    fn render_icon(
        &self,
        highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let color = appearance.theme().foreground().into_solid();
        render_search_item_icon(appearance, Icon::Terminal, color, highlight_state)
    }

    fn render_item(
        &self,
        highlight_state: ItemHighlightState,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let background = highlight_state
            .container_background_fill(appearance)
            .unwrap_or_else(|| appearance.theme().surface_2());
        let font_family = appearance.ui_font_family();
        let font_size = appearance.monospace_font_size();

        let highlight = Highlight::new()
            .with_properties(Properties::default().weight(Weight::Bold))
            .with_foreground_color(appearance.theme().main_text_color(background).into_solid());
        let alias = Text::new_inline(self.alias.clone(), font_family, font_size)
            .with_color(appearance.theme().sub_text_color(background).into_solid())
            .with_single_highlight(highlight, self.match_result.matched_indices.clone())
            .finish();

        let mut row = Flex::row();
        row.add_child(Shrinkable::new(1., Align::new(alias).left().finish()).finish());
        if !self.tags.is_empty() {
            let tags = Text::new_inline(self.tags.join("  "), font_family, font_size)
                .with_color(appearance.theme().hint_text_color(background).into_solid())
                .finish();
            row.add_child(
                Container::new(tags)
                    .with_margin_right(TAGS_MARGIN_RIGHT)
                    .finish(),
            );
        }
        ConstrainedBox::new(row.finish())
            .with_height(ROW_HEIGHT)
            .finish()
    }

    fn score(&self) -> OrderedFloat<f64> {
        OrderedFloat::from(self.match_result.score as f64)
    }

    fn accept_result(&self) -> Self::Action {
        self.connect()
    }

    fn execute_result(&self) -> Self::Action {
        self.connect()
    }

    fn accessibility_label(&self) -> String {
        format!("Selected {}.", self.alias)
    }

    fn accessibility_help_message(&self) -> Option<String> {
        Some("Press enter to connect to this server.".into())
    }
}
