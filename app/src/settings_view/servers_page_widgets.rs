//! The sections of the Servers settings page.
use warpui::elements::{
    Align, ChildView, Container, CornerRadius, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    Hoverable, MainAxisSize, ParentElement, Radius, Text,
};
use warpui::platform::Cursor;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{AppContext, SingletonEntity, ViewHandle};

use super::ToggleState;
use super::servers_page::{
    MAX_ROWS, ServersPageAction, ServersSettingsPageView, Status, root_login_label, transport_label,
};
use super::settings_page::{
    LocalOnlyIconState, SettingsWidget, render_body_item, render_sub_header_with_description,
};
use crate::appearance::Appearance;
use crate::editor::EditorView;
use crate::host_directory::{self, Host, HostDirectoryModel, HostSource};

const INPUT_WIDTH: f32 = 320.;
const INPUT_VERTICAL_PADDING: f32 = 7.;
const INPUT_HORIZONTAL_PADDING: f32 = 12.;
const ROW_FONT_SIZE: f32 = 13.;
const ROW_RADIUS: f32 = 4.;

fn text_input(editor: &ViewHandle<EditorView>, appearance: &Appearance) -> Box<dyn Element> {
    appearance
        .ui_builder()
        .text_input(editor.clone())
        .with_style(UiComponentStyles {
            width: Some(INPUT_WIDTH),
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

fn button(
    label: &str,
    variant: ButtonVariant,
    mouse_state: warpui::elements::MouseStateHandle,
    action: ServersPageAction,
    enabled: bool,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let button = appearance
        .ui_builder()
        .button(variant, mouse_state)
        .with_text_label(label.to_owned());
    if !enabled {
        return button.disabled().build().finish();
    }
    button
        .build()
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
        .finish()
}

fn note(text: String, is_error: bool, appearance: &Appearance) -> Box<dyn Element> {
    let theme = appearance.theme();
    let color = if is_error {
        theme.ui_error_color()
    } else {
        theme.nonactive_ui_text_color().into()
    };
    Container::new(
        Align::new(
            Text::new(text, appearance.ui_font_family(), ROW_FONT_SIZE)
                .with_color(color)
                .finish(),
        )
        .left()
        .finish(),
    )
    .with_padding_bottom(8.)
    .finish()
}

fn status_note(status: &Option<Status>, appearance: &Appearance) -> Option<Box<dyn Element>> {
    status
        .as_ref()
        .map(|status| note(status.text.clone(), status.is_error, appearance))
}

fn field(
    label: &str,
    editor: &ViewHandle<EditorView>,
    description: Option<&str>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    render_body_item::<ServersPageAction>(
        label.to_owned(),
        None,
        LocalOnlyIconState::Hidden,
        ToggleState::Enabled,
        appearance,
        text_input(editor, appearance),
        description.map(str::to_owned),
    )
}

/// Tells the person that `ssh <alias>` outside Warp does not see their Warp servers yet.
pub(super) struct IncludeBannerWidget;

impl SettingsWidget for IncludeBannerWidget {
    type View = ServersSettingsPageView;

    fn search_terms(&self) -> &str {
        "servers ssh config include warp.conf"
    }

    fn should_render(&self, _app: &AppContext) -> bool {
        true
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        if !view.include_missing {
            return Empty::new().finish();
        }
        render_body_item::<ServersPageAction>(
            "Let ssh read Warp's servers".into(),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            button(
                "Add the line",
                ButtonVariant::Accent,
                view.buttons.install_include.clone(),
                ServersPageAction::InstallInclude,
                !view.busy,
                appearance,
            ),
            Some(
                "Servers added here are kept in ~/.ssh/config.d/warp.conf. Until \
                 ~/.ssh/config reads that file, `ssh <name>` only works from Warp. This adds \
                 the line `Include config.d/warp.conf` at the top of ~/.ssh/config and saves \
                 the previous file next to it as config.warp-backup-<time>. Nothing else in \
                 the file changes."
                    .to_owned(),
            ),
        )
    }
}

pub(super) struct ServersListWidget;

impl SettingsWidget for ServersListWidget {
    type View = ServersSettingsPageView;

    fn search_terms(&self) -> &str {
        "servers hosts ssh list search tags import connect"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let query = view.search.as_ref(app).buffer_text(app);
        let hosts = HostDirectoryModel::as_ref(app).hosts();
        let found = host_directory::search_all(hosts, &query);

        let mut column = Flex::column()
            .with_child(render_sub_header_with_description(
                appearance,
                "Servers",
                format!(
                    "{} in the list. Servers come from your ~/.ssh/config or are added here.",
                    count_label(hosts.len())
                ),
            ))
            .with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(text_input(&view.search, appearance))
                        .with_child(
                            Container::new(button(
                                "Import from SSH config",
                                ButtonVariant::Secondary,
                                view.buttons.import.clone(),
                                ServersPageAction::ImportFromSshConfig,
                                true,
                                appearance,
                            ))
                            .with_margin_left(12.)
                            .finish(),
                        )
                        .finish(),
                )
                .with_padding_bottom(8.)
                .finish(),
            );

        for host in found.iter().take(MAX_ROWS) {
            column.add_child(render_row(view, host, appearance));
        }
        if found.len() > MAX_ROWS {
            column.add_child(note(
                format!(
                    "{} more; type in the search box to narrow the list.",
                    found.len() - MAX_ROWS
                ),
                false,
                appearance,
            ));
        } else if found.is_empty() {
            column.add_child(note("No server matches.".to_owned(), false, appearance));
        }
        Container::new(column.finish())
            .with_padding_bottom(16.)
            .finish()
    }
}

fn count_label(count: usize) -> String {
    match count {
        1 => "1 server".to_owned(),
        count => format!("{count} servers"),
    }
}

fn render_row(
    view: &ServersSettingsPageView,
    host: &Host,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let is_selected = view.selected.as_deref() == Some(host.alias.as_str());
    let mouse_state = view
        .row_states
        .borrow_mut()
        .entry(host.alias.clone())
        .or_default()
        .clone();
    let target = match view.resolved.get(&host.alias) {
        _ if host.missing => "not in SSH config".to_owned(),
        Some(Ok(resolved)) => resolved.display(),
        Some(Err(_)) => "cannot be resolved".to_owned(),
        None => "…".to_owned(),
    };
    let mut details = vec![target];
    if host.source == HostSource::Warp {
        details.push("added in Warp".to_owned());
    }
    if !host.tags.is_empty() {
        details.push(host.tags.join(", "));
    }

    let alias = host.alias.clone();
    let text_color = theme.active_ui_text_color();
    let sub_color = theme.nonactive_ui_text_color();
    let selected_background = theme.surface_2();
    let hover_background = theme.surface_1();
    let font_family = appearance.ui_font_family();
    let font_size = appearance.ui_font_size();
    let label = host.alias.clone();

    Hoverable::new(mouse_state, move |state| {
        let mut row = Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        Text::new_inline(label.clone(), font_family, font_size)
                            .with_color(text_color.into())
                            .finish(),
                    )
                    .with_margin_right(16.)
                    .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.,
                        Text::new_inline(details.join("  ·  "), font_family, ROW_FONT_SIZE)
                            .with_color(sub_color.into())
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
        )
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(ROW_RADIUS)))
        .with_horizontal_padding(12.)
        .with_vertical_padding(7.);
        if is_selected {
            row = row.with_background(selected_background);
        } else if state.is_hovered() {
            row = row.with_background(hover_background);
        }
        row.finish()
    })
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action(ServersPageAction::Select(alias.clone()));
    })
    .with_cursor(Cursor::PointingHand)
    .finish()
}

pub(super) struct ServerDetailWidget;

impl SettingsWidget for ServerDetailWidget {
    type View = ServersSettingsPageView;

    fn search_terms(&self) -> &str {
        "servers host tags root sudo transport forget delete"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let Some(host) = view.selected_host(app) else {
            return Empty::new().finish();
        };
        let mut column = Flex::column().with_child(render_sub_header_with_description(
            appearance,
            host.alias.clone(),
            detail_description(view, &host),
        ));
        column.add_child(field(
            "Tags",
            &view.tags,
            Some("Words that group servers, separated by commas. Search with tag:prod."),
            appearance,
        ));
        column.add_child(render_body_item::<ServersPageAction>(
            "Becoming root".into(),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            ChildView::new(&view.root_login).finish(),
            Some(format!(
                "How you get a root shell after signing in (now: {}). Warp only records this.",
                root_login_label(host.root_login)
            )),
        ));
        column.add_child(render_body_item::<ServersPageAction>(
            "Running commands".into(),
            None,
            LocalOnlyIconState::Hidden,
            ToggleState::Enabled,
            appearance,
            ChildView::new(&view.transport).finish(),
            Some(format!(
                "Which channel commands use (now: {}). Warp only records this.",
                transport_label(host.transport)
            )),
        ));
        column.add_child(remove_row(view, &host, appearance));
        if let Some(status) = status_note(&view.status, appearance) {
            column.add_child(status);
        }
        Container::new(column.finish())
            .with_padding_bottom(16.)
            .finish()
    }
}

fn detail_description(view: &ServersSettingsPageView, host: &Host) -> String {
    let origin = match (host.source, host.missing) {
        (_, true) => "No longer in your SSH config; Warp keeps its notes until you forget it.",
        (HostSource::SshConfig, false) => "From your SSH config; Warp never changes those lines.",
        (HostSource::Warp, false) => "Added in Warp, stored in ~/.ssh/config.d/warp.conf.",
    };
    match view.resolved.get(&host.alias) {
        Some(Ok(resolved)) => format!("{} {origin}", resolved.display()),
        Some(Err(reason)) => format!("{origin} ssh could not resolve it: {reason}"),
        None => origin.to_owned(),
    }
}

fn remove_row(
    view: &ServersSettingsPageView,
    host: &Host,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let (label, description) = match host.source {
        HostSource::Warp => (
            "Delete",
            "Removes the server from ~/.ssh/config.d/warp.conf and from this list.",
        ),
        HostSource::SshConfig => (
            "Forget",
            "Drops what Warp knows about the server. Your SSH config is not touched, so the \
             server comes back untagged while it is still there.",
        ),
    };
    let control = if view.confirming_remove {
        Flex::row()
            .with_child(button(
                &format!("{label} {}", host.alias),
                ButtonVariant::Error,
                view.buttons.confirm_remove.clone(),
                ServersPageAction::ConfirmRemove,
                !view.busy,
                appearance,
            ))
            .with_child(
                Container::new(button(
                    "Cancel",
                    ButtonVariant::Secondary,
                    view.buttons.cancel_remove.clone(),
                    ServersPageAction::CancelRemove,
                    true,
                    appearance,
                ))
                .with_margin_left(8.)
                .finish(),
            )
            .finish()
    } else {
        button(
            label,
            ButtonVariant::Warn,
            view.buttons.remove.clone(),
            ServersPageAction::ArmRemove,
            !view.busy,
            appearance,
        )
    };
    render_body_item::<ServersPageAction>(
        label.into(),
        None,
        LocalOnlyIconState::Hidden,
        ToggleState::Enabled,
        appearance,
        control,
        Some(description.to_owned()),
    )
}

pub(super) struct AddServerWidget;

impl SettingsWidget for AddServerWidget {
    type View = ServersSettingsPageView;

    fn search_terms(&self) -> &str {
        "add server new host name user port key identity jump proxy ssh"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let add = &view.add;
        let mut column = Flex::column()
            .with_child(render_sub_header_with_description(
                appearance,
                "Add a server",
                "Written to ~/.ssh/config.d/warp.conf. Warp checks with ssh that it reads back \
                 as entered before keeping it.",
            ))
            .with_child(field(
                "Name",
                &add.alias,
                Some("What you type after ssh."),
                appearance,
            ))
            .with_child(field("Host name", &add.hostname, None, appearance))
            .with_child(field("User", &add.user, None, appearance))
            .with_child(field("Port", &add.port, None, appearance))
            .with_child(field("Key file", &add.identity_file, None, appearance))
            .with_child(field("Jump host", &add.proxy_jump, None, appearance))
            .with_child(field("Tags", &add.tags, None, appearance))
            .with_child(render_body_item::<ServersPageAction>(
                "Becoming root".into(),
                None,
                LocalOnlyIconState::Hidden,
                ToggleState::Enabled,
                appearance,
                ChildView::new(&add.root_login).finish(),
                None,
            ))
            .with_child(
                Container::new(button(
                    if view.busy {
                        "Working…"
                    } else {
                        "Add server"
                    },
                    ButtonVariant::Accent,
                    view.buttons.create.clone(),
                    ServersPageAction::CreateServer,
                    !view.busy,
                    appearance,
                ))
                .with_padding_bottom(8.)
                .finish(),
            );
        if view.selected.is_none()
            && let Some(status) = status_note(&view.status, appearance)
        {
            column.add_child(status);
        }
        column.finish()
    }
}
