//! Settings UI for the server directory: the list of servers, what Warp knows about the selected
//! one, and a form to add a server to Warp's own part of the SSH configuration.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use warpui::elements::{Element, MouseStateHandle};
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle};

use super::SettingsSection;
use super::servers_page_widgets::{
    AddServerWidget, IncludeBannerWidget, ServerDetailWidget, ServersListWidget,
};
use super::settings_page::{
    MatchData, PageTitle, PageType, SettingsPageEvent, SettingsPageMeta, SettingsPageViewHandle,
    SettingsWidget,
};
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, InteractionState, SingleLineEditorOptions, TextOptions,
};
use crate::host_directory::{
    self, Host, HostDirectoryEvent, HostDirectoryModel, HostEdit, HostSource, NewHost,
    ProvisionError, RefreshMode, Resolved, RootLogin, SshResolver, SystemSsh, Transport,
};
use crate::view_components::{Dropdown, DropdownItem};
use crate::warp_sync::SyncConfig;

/// Rows shown at once; the search narrows a longer list.
pub(super) const MAX_ROWS: usize = 40;

#[derive(Clone, Debug, PartialEq)]
pub enum ServersPageAction {
    Select(String),
    ImportFromSshConfig,
    CreateServer,
    SetAddRootLogin(RootLogin),
    SetRootLogin(RootLogin),
    SetTransport(Transport),
    ArmRemove,
    CancelRemove,
    ConfirmRemove,
    InstallInclude,
    OpenMirror,
}

/// A message under the form or the detail, until the next thing the person does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Status {
    pub(super) text: String,
    pub(super) is_error: bool,
}

/// The editors of the "Add server" form, by field.
pub(super) struct AddForm {
    pub(super) alias: ViewHandle<EditorView>,
    pub(super) hostname: ViewHandle<EditorView>,
    pub(super) user: ViewHandle<EditorView>,
    pub(super) port: ViewHandle<EditorView>,
    pub(super) identity_file: ViewHandle<EditorView>,
    pub(super) proxy_jump: ViewHandle<EditorView>,
    pub(super) tags: ViewHandle<EditorView>,
    pub(super) root_login: ViewHandle<Dropdown<ServersPageAction>>,
}

impl AddForm {
    fn editors(&self) -> [&ViewHandle<EditorView>; 7] {
        [
            &self.alias,
            &self.hostname,
            &self.user,
            &self.port,
            &self.identity_file,
            &self.proxy_jump,
            &self.tags,
        ]
    }
}

/// The mouse state of every button on the page, created once so that clicks are tracked.
#[derive(Default)]
pub(super) struct ButtonStates {
    pub(super) import: MouseStateHandle,
    pub(super) create: MouseStateHandle,
    pub(super) remove: MouseStateHandle,
    pub(super) confirm_remove: MouseStateHandle,
    pub(super) cancel_remove: MouseStateHandle,
    pub(super) install_include: MouseStateHandle,
    pub(super) open_mirror: MouseStateHandle,
}

pub struct ServersSettingsPageView {
    page: PageType<Self>,
    pub(super) search: ViewHandle<EditorView>,
    pub(super) tags: ViewHandle<EditorView>,
    pub(super) add: AddForm,
    pub(super) root_login: ViewHandle<Dropdown<ServersPageAction>>,
    pub(super) transport: ViewHandle<Dropdown<ServersPageAction>>,
    pub(super) add_root_login: RootLogin,
    pub(super) selected: Option<String>,
    pub(super) resolved: HashMap<String, Result<Resolved, String>>,
    resolving: HashSet<String>,
    pub(super) confirming_remove: bool,
    pub(super) include_missing: bool,
    pub(super) busy: bool,
    pub(super) status: Option<Status>,
    pub(super) row_states: RefCell<HashMap<String, MouseStateHandle>>,
    pub(super) buttons: ButtonStates,
}

impl ServersSettingsPageView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let search = Self::editor("Search by name or tag, e.g. web or tag:prod", ctx);
        ctx.subscribe_to_view(&search, |me, _, event, ctx| match event {
            EditorEvent::Edited(_) => me.resolve_visible(ctx),
            EditorEvent::Escape => ctx.emit(SettingsPageEvent::FocusModal),
            _ => {}
        });
        let tags = Self::editor("prod, web", ctx);
        ctx.subscribe_to_view(&tags, |me, _, event, ctx| match event {
            EditorEvent::Blurred | EditorEvent::Enter => me.save_tags(ctx),
            EditorEvent::Escape => ctx.emit(SettingsPageEvent::FocusModal),
            _ => {}
        });

        let add = AddForm {
            alias: Self::editor("lab-1", ctx),
            hostname: Self::editor("203.0.113.10 or host.example.com", ctx),
            user: Self::editor("optional", ctx),
            port: Self::editor("22", ctx),
            identity_file: Self::editor("~/.ssh/id_ed25519", ctx),
            proxy_jump: Self::editor("optional: user@jump.example.com", ctx),
            tags: Self::editor("prod, web", ctx),
            root_login: Self::dropdown(ctx),
        };
        for editor in add.editors() {
            ctx.subscribe_to_view(editor, |me, _, event, ctx| match event {
                EditorEvent::Enter => me.create_server(ctx),
                EditorEvent::Escape => ctx.emit(SettingsPageEvent::FocusModal),
                _ => {}
            });
        }

        let root_login = Self::dropdown(ctx);
        let transport = Self::dropdown(ctx);
        Self::set_root_login_items(&add.root_login, ServersPageAction::SetAddRootLogin, ctx);
        Self::set_root_login_items(&root_login, ServersPageAction::SetRootLogin, ctx);
        add.root_login.update(ctx, |dropdown, ctx| {
            dropdown.set_selected_by_action(
                ServersPageAction::SetAddRootLogin(RootLogin::default()),
                ctx,
            );
        });
        transport.update(ctx, |dropdown, ctx| {
            dropdown.set_items(
                [Transport::InBand, Transport::Direct]
                    .into_iter()
                    .map(|choice| {
                        DropdownItem::new(
                            transport_label(choice),
                            ServersPageAction::SetTransport(choice),
                        )
                    })
                    .collect(),
                ctx,
            );
        });

        ctx.subscribe_to_model(&HostDirectoryModel::handle(ctx), |me, _, event, ctx| {
            if matches!(event, HostDirectoryEvent::Changed) {
                me.on_hosts_changed(ctx);
            }
        });

        let widgets: Vec<Box<dyn SettingsWidget<View = Self>>> = vec![
            Box::new(IncludeBannerWidget),
            Box::new(ServersListWidget),
            Box::new(ServerDetailWidget),
            Box::new(AddServerWidget),
        ];
        let mut view = Self {
            page: PageType::new_uncategorized(widgets, Some(PageTitle::new("Servers"))),
            search,
            tags,
            add,
            root_login,
            transport,
            add_root_login: RootLogin::default(),
            selected: None,
            resolved: HashMap::new(),
            resolving: HashSet::new(),
            confirming_remove: false,
            include_missing: false,
            busy: false,
            status: None,
            row_states: RefCell::new(HashMap::new()),
            buttons: ButtonStates::default(),
        };
        view.on_hosts_changed(ctx);
        view
    }

    fn editor(placeholder: &str, ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        let editor = ctx.add_typed_action_view(|ctx| {
            EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions {
                        font_size_override: Some(Appearance::as_ref(ctx).ui_font_size()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ctx,
            )
        });
        editor.update(ctx, |editor, ctx| {
            editor.set_placeholder_text(placeholder, ctx)
        });
        editor
    }

    fn dropdown(ctx: &mut ViewContext<Self>) -> ViewHandle<Dropdown<ServersPageAction>> {
        ctx.add_typed_action_view(|ctx| {
            let mut dropdown = Dropdown::new(ctx);
            dropdown.set_top_bar_max_width(240.);
            dropdown
        })
    }

    fn set_root_login_items(
        dropdown: &ViewHandle<Dropdown<ServersPageAction>>,
        action: fn(RootLogin) -> ServersPageAction,
        ctx: &mut ViewContext<Self>,
    ) {
        dropdown.update(ctx, |dropdown, ctx| {
            dropdown.set_items(
                ROOT_LOGINS
                    .into_iter()
                    .map(|choice| DropdownItem::new(root_login_label(choice), action(choice)))
                    .collect(),
                ctx,
            );
        });
    }

    fn hosts(ctx: &AppContext) -> Vec<Host> {
        HostDirectoryModel::as_ref(ctx).hosts().to_vec()
    }

    pub(super) fn selected_host(&self, ctx: &AppContext) -> Option<Host> {
        let alias = self.selected.as_deref()?;
        HostDirectoryModel::as_ref(ctx)
            .hosts()
            .iter()
            .find(|host| host.alias == alias)
            .cloned()
    }

    fn on_hosts_changed(&mut self, ctx: &mut ViewContext<Self>) {
        let selected = self.selected_host(ctx);
        if selected.is_none() {
            self.selected = None;
        }
        self.show_selected(selected.as_ref(), ctx);
        self.refresh_include_state(ctx);
        self.resolve_visible(ctx);
        ctx.notify();
    }

    /// Puts what Warp knows about `host` into the detail.
    fn show_selected(&mut self, host: Option<&Host>, ctx: &mut ViewContext<Self>) {
        let Some(host) = host else {
            return;
        };
        let tags = host.tags.join(", ");
        if self.tags.as_ref(ctx).buffer_text(ctx) != tags {
            self.tags.update(ctx, |editor, ctx| {
                editor.set_interaction_state(InteractionState::Editable, ctx);
                editor.set_buffer_text(&tags, ctx);
            });
        }
        self.root_login.update(ctx, |dropdown, ctx| {
            dropdown.set_selected_by_action(ServersPageAction::SetRootLogin(host.root_login), ctx);
        });
        self.transport.update(ctx, |dropdown, ctx| {
            dropdown.set_selected_by_action(ServersPageAction::SetTransport(host.transport), ctx);
        });
    }

    fn select(&mut self, alias: &str, ctx: &mut ViewContext<Self>) {
        self.selected = Some(alias.to_owned());
        self.confirming_remove = false;
        self.status = None;
        let host = self.selected_host(ctx);
        self.show_selected(host.as_ref(), ctx);
        self.resolve_visible(ctx);
        ctx.notify();
    }

    /// Asks OpenSSH, in the background, what the selected host and the rows in view resolve to.
    fn resolve_visible(&mut self, ctx: &mut ViewContext<Self>) {
        let query = self.search.as_ref(ctx).buffer_text(ctx);
        let hosts = Self::hosts(ctx);
        let home = dirs::home_dir();
        let selected = self.selected.clone();
        let wanted: Vec<(String, Option<PathBuf>)> = selected
            .iter()
            .filter_map(|alias| hosts.iter().find(|host| &host.alias == alias))
            .chain(
                host_directory::search_all(&hosts, &query)
                    .into_iter()
                    .take(MAX_ROWS),
            )
            .filter(|host| !host.missing)
            .filter(|host| {
                !self.resolved.contains_key(&host.alias) && !self.resolving.contains(&host.alias)
            })
            .map(|host| {
                // A host Warp created is read from Warp's file alone, so that it resolves before
                // the user's configuration includes that file.
                let config = home
                    .as_deref()
                    .filter(|_| host.source == HostSource::Warp)
                    .map(host_directory::warp_conf_path);
                (host.alias.clone(), config)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .fold(Vec::new(), |mut unique, entry| {
                if !unique.iter().any(|(alias, _)| alias == &entry.0) {
                    unique.push(entry);
                }
                unique
            });
        if wanted.is_empty() {
            return;
        }
        self.resolving
            .extend(wanted.iter().map(|(alias, _)| alias.clone()));
        ctx.spawn(
            async move {
                wanted
                    .into_iter()
                    .map(|(alias, config)| {
                        let result = SystemSsh.resolve(config.as_deref(), &alias);
                        (alias, result)
                    })
                    .collect::<Vec<_>>()
            },
            |me, results, ctx| {
                for (alias, result) in results {
                    me.resolving.remove(&alias);
                    me.resolved.insert(alias, result);
                }
                ctx.notify();
            },
        );
    }

    /// Whether the user's SSH configuration reads Warp's file, which matters once a host lives there.
    fn refresh_include_state(&mut self, ctx: &AppContext) {
        let has_warp_host = HostDirectoryModel::as_ref(ctx)
            .hosts()
            .iter()
            .any(|host| host.source == HostSource::Warp);
        self.include_missing = has_warp_host
            && dirs::home_dir().is_some_and(|home| !host_directory::include_installed(&home));
    }

    fn set_status(&mut self, text: impl Into<String>, is_error: bool) {
        self.status = Some(Status {
            text: text.into(),
            is_error,
        });
    }

    /// Runs a change to the files on disk in the background and, when it succeeds, makes its
    /// result the list of hosts.
    fn run_change(
        &mut self,
        work: impl FnOnce(&std::path::Path) -> Result<Vec<Host>, ProvisionError> + Send + 'static,
        on_success: impl FnOnce(&mut Self, &mut ViewContext<Self>) + 'static,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(home) = dirs::home_dir() else {
            self.set_status("The home directory is not known", true);
            ctx.notify();
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = None;
        ctx.notify();
        ctx.spawn(async move { work(&home) }, move |me, result, ctx| {
            me.busy = false;
            match result {
                Ok(hosts) => {
                    HostDirectoryModel::handle(ctx)
                        .update(ctx, |model, ctx| model.replace_hosts(hosts, ctx));
                    on_success(me, ctx);
                }
                Err(error) => me.set_status(error.to_string(), true),
            }
            ctx.notify();
        });
    }

    fn create_server(&mut self, ctx: &mut ViewContext<Self>) {
        let new = match self.read_add_form(ctx) {
            Ok(new) => new,
            Err(reason) => {
                self.set_status(reason, true);
                ctx.notify();
                return;
            }
        };
        let root_login = self.add_root_login;
        let alias = new.alias.clone();
        self.run_change(
            move |home| host_directory::create_host(home, &new, root_login, &SystemSsh),
            move |me, ctx| {
                for editor in me.add.editors() {
                    editor.update(ctx, |editor, ctx| editor.clear_buffer(ctx));
                }
                me.resolved.remove(&alias);
                me.set_status(format!("Added {alias}"), false);
                me.select(&alias, ctx);
            },
            ctx,
        );
    }

    fn read_add_form(&self, ctx: &AppContext) -> Result<NewHost, String> {
        let text = |editor: &ViewHandle<EditorView>| editor.as_ref(ctx).buffer_text(ctx);
        build_new_host(&AddFormText {
            alias: text(&self.add.alias),
            hostname: text(&self.add.hostname),
            user: text(&self.add.user),
            port: text(&self.add.port),
            identity_file: text(&self.add.identity_file),
            proxy_jump: text(&self.add.proxy_jump),
            tags: text(&self.add.tags),
        })
    }

    fn save_tags(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(host) = self.selected_host(ctx) else {
            return;
        };
        let input = self.tags.as_ref(ctx).buffer_text(ctx);
        match host_directory::parse_tags(&input) {
            Ok(tags) if tags != host.tags => {
                self.edit_selected(&host, |edit| edit.tags = tags, ctx);
            }
            Ok(_) => {}
            Err(reason) => {
                self.set_status(reason, true);
                ctx.notify();
            }
        }
    }

    fn edit_selected(
        &mut self,
        host: &Host,
        change: impl FnOnce(&mut HostEdit),
        ctx: &mut ViewContext<Self>,
    ) {
        let mut edit = HostEdit {
            tags: host.tags.clone(),
            root_login: host.root_login,
            transport: host.transport,
        };
        change(&mut edit);
        let alias = host.alias.clone();
        self.run_change(
            move |home| host_directory::update_host(home, &alias, &edit),
            |_, _| {},
            ctx,
        );
    }

    fn remove_selected(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(host) = self.selected_host(ctx) else {
            return;
        };
        let alias = host.alias.clone();
        let verb = match host.source {
            HostSource::Warp => "Deleted",
            HostSource::SshConfig => "Forgot",
        };
        self.confirming_remove = false;
        self.run_change(
            move |home| host_directory::remove_host(home, &alias),
            move |me, _| {
                me.selected = None;
                me.set_status(format!("{verb} {}", host.alias), false);
            },
            ctx,
        );
    }

    /// Shows the folder that Warp Sync mirrors the selected server into.
    fn open_mirror(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(host) = self.selected_host(ctx) else {
            return;
        };
        let opened = SyncConfig::from_settings(ctx)
            .map_err(|error| error.to_string())
            .and_then(|config| {
                host_directory::mirror_dir(&host, &config.mirror_root)
                    .filter(|dir| dir.is_dir())
                    .ok_or_else(|| {
                        "There is no local mirror of this server yet: download a file from it \
                         with Warp Sync first."
                            .to_owned()
                    })
            });
        match opened {
            Ok(dir) => {
                self.status = None;
                ctx.open_file_path_in_explorer(&dir);
            }
            Err(text) => self.set_status(text, true),
        }
        ctx.notify();
    }

    fn install_include(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = None;
        ctx.notify();
        ctx.spawn(
            async move { host_directory::install_include(&home) },
            |me, result, ctx| {
                me.busy = false;
                match result {
                    Ok(backup) => {
                        let saved = backup.map_or_else(String::new, |path| {
                            format!("; the previous file is saved as {}", path.display())
                        });
                        me.set_status(format!("Added the line to ~/.ssh/config{saved}"), false);
                    }
                    Err(error) => me.set_status(error.to_string(), true),
                }
                me.refresh_include_state(ctx);
                ctx.notify();
            },
        );
    }
}

/// What was typed in the form.
#[derive(Debug, Clone, Default)]
pub(super) struct AddFormText {
    pub(super) alias: String,
    pub(super) hostname: String,
    pub(super) user: String,
    pub(super) port: String,
    pub(super) identity_file: String,
    pub(super) proxy_jump: String,
    pub(super) tags: String,
}

/// The host the form describes, or what is wrong with it. Empty optional fields are left out.
pub(super) fn build_new_host(form: &AddFormText) -> Result<NewHost, String> {
    let optional = |text: &str| {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    };
    let port = match form.port.trim() {
        "" => None,
        text => Some(
            text.parse::<u16>()
                .map_err(|_| "The port must be a number from 1 to 65535".to_owned())?,
        ),
    };
    let new = NewHost {
        alias: form.alias.trim().to_owned(),
        hostname: form.hostname.trim().to_owned(),
        user: optional(&form.user),
        port,
        identity_file: optional(&form.identity_file),
        proxy_jump: optional(&form.proxy_jump),
        tags: host_directory::parse_tags(&form.tags)?,
    };
    host_directory::validate_new_host(&new)?;
    Ok(new)
}

const ROOT_LOGINS: [RootLogin; 4] = [
    RootLogin::None,
    RootLogin::Root,
    RootLogin::SudoNopasswd,
    RootLogin::SudoPassword,
];

pub(super) fn root_login_label(choice: RootLogin) -> &'static str {
    match choice {
        RootLogin::None => "Not set",
        RootLogin::Root => "The SSH user is root",
        RootLogin::SudoNopasswd => "sudo, no password",
        RootLogin::SudoPassword => "sudo, with password",
    }
}

pub(super) fn transport_label(choice: Transport) -> &'static str {
    match choice {
        Transport::InBand => "Through the terminal",
        Transport::Direct => "Direct SSH connection",
    }
}

impl Entity for ServersSettingsPageView {
    type Event = SettingsPageEvent;
}

impl TypedActionView for ServersSettingsPageView {
    type Action = ServersPageAction;

    fn handle_action(&mut self, action: &ServersPageAction, ctx: &mut ViewContext<Self>) {
        match action {
            ServersPageAction::Select(alias) => self.select(alias, ctx),
            ServersPageAction::ImportFromSshConfig => {
                self.resolved.clear();
                HostDirectoryModel::handle(ctx)
                    .update(ctx, |model, ctx| model.refresh(RefreshMode::Manual, ctx));
            }
            ServersPageAction::CreateServer => self.create_server(ctx),
            ServersPageAction::SetAddRootLogin(choice) => {
                self.add_root_login = *choice;
                ctx.notify();
            }
            ServersPageAction::SetRootLogin(choice) => {
                if let Some(host) = self.selected_host(ctx)
                    && host.root_login != *choice
                {
                    self.edit_selected(&host, |edit| edit.root_login = *choice, ctx);
                }
            }
            ServersPageAction::SetTransport(choice) => {
                if let Some(host) = self.selected_host(ctx)
                    && host.transport != *choice
                {
                    self.edit_selected(&host, |edit| edit.transport = *choice, ctx);
                }
            }
            ServersPageAction::ArmRemove => {
                self.confirming_remove = true;
                ctx.notify();
            }
            ServersPageAction::CancelRemove => {
                self.confirming_remove = false;
                ctx.notify();
            }
            ServersPageAction::ConfirmRemove => self.remove_selected(ctx),
            ServersPageAction::InstallInclude => self.install_include(ctx),
            ServersPageAction::OpenMirror => self.open_mirror(ctx),
        }
    }
}

impl View for ServersSettingsPageView {
    fn ui_name() -> &'static str {
        "ServersSettingsPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

impl SettingsPageMeta for ServersSettingsPageView {
    fn section() -> SettingsSection {
        SettingsSection::Servers
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        cfg!(not(target_family = "wasm")) && host_directory::is_enabled()
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

impl From<ViewHandle<ServersSettingsPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<ServersSettingsPageView>) -> Self {
        SettingsPageViewHandle::Servers(view_handle)
    }
}

#[cfg(test)]
#[path = "servers_page_tests.rs"]
mod tests;
