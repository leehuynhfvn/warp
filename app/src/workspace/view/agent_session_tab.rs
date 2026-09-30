//! The tab an agent's own SSH session lives in: opened where the user can see it, without taking
//! the keyboard away from the tab they were in, and kept together in an "Agents" tab group.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use warp_core::features::FeatureFlag;
use warpui::{ViewContext, ViewHandle};

use super::Workspace;
use crate::host_directory::validate_alias;
use crate::launch_configs::launch_config::{CommandTemplate, PaneMode, PaneTemplateType};
use crate::pane_group::{PaneId, PanesLayout};
use crate::terminal::view::TerminalView;
use crate::workspace::tab_group::TabGroup;

/// Name of the tab group that holds the tabs agents open.
pub(crate) const AGENTS_GROUP_NAME: &str = "Agents";

/// Prefix of the title of a tab an agent opened.
const TAB_TITLE_PREFIX: &str = "Agent · ";

/// The pane an agent's session will appear in.
pub(crate) struct AgentSessionTab {
    pub(crate) pane_id: PaneId,
    pub(crate) terminal_view: ViewHandle<TerminalView>,
}

impl Workspace {
    /// Opens a tab that signs in to `alias` the way the quick connect does (the alias is typed
    /// into the shell without quoting, so it is checked again here) and leaves the user in the
    /// tab they were in. The tab goes into the "Agents" group, which is created if needed and
    /// always shown expanded, when tab groups are available.
    pub(crate) fn open_agent_session_tab(
        &mut self,
        alias: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<AgentSessionTab, String> {
        validate_alias(alias).map_err(|reason| format!("the alias \"{alias}\" {reason}"))?;
        let previous_tab = self
            .tabs
            .get(self.active_tab_index)
            .map(|tab| tab.pane_group.id());

        let pane = PaneTemplateType::PaneTemplate {
            cwd: PathBuf::new(),
            commands: vec![CommandTemplate {
                exec: format!("ssh {alias}"),
            }],
            is_focused: Some(true),
            pane_mode: PaneMode::Terminal,
            shell: None,
        };
        self.add_tab_with_pane_layout(
            PanesLayout::Template(pane),
            Arc::new(HashMap::new()),
            Some(format!("{TAB_TITLE_PREFIX}{alias}")),
            ctx,
        );
        let new_index = self.active_tab_index;
        let new_tab = self
            .tabs
            .get(new_index)
            .map(|tab| (tab.pane_group.id(), tab.pane_group.clone()));
        let Some((new_tab_id, pane_group)) = new_tab else {
            return Err("the new tab could not be found".to_owned());
        };

        self.place_in_agents_group(new_index, ctx);
        if let Some(previous_tab) = previous_tab
            && previous_tab != new_tab_id
        {
            self.activate_tab_by_pane_group_id(previous_tab, ctx);
        }

        let found = pane_group.read(ctx, |pane_group, ctx| {
            let pane_id = pane_group.focused_pane_id(ctx);
            pane_group
                .terminal_view_from_pane_id(pane_id, ctx)
                .map(|terminal_view| (pane_id, terminal_view))
        });
        let Some((pane_id, terminal_view)) = found else {
            return Err("the new tab has no terminal".to_owned());
        };
        Ok(AgentSessionTab {
            pane_id,
            terminal_view,
        })
    }

    /// Moves the tab at `index` into the "Agents" group and makes sure the group is expanded. Does
    /// nothing when tab groups are not available.
    fn place_in_agents_group(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        if !FeatureFlag::GroupedTabs.is_enabled() {
            return;
        }
        let existing = self
            .tab_groups
            .values()
            .find(|group| group.name.as_deref() == Some(AGENTS_GROUP_NAME))
            .map(|group| group.id);
        let (group_id, target) = match existing {
            Some(group_id) => (
                group_id,
                self.index_after_group(group_id).unwrap_or(self.tabs.len()),
            ),
            None => {
                let group = TabGroup {
                    name: Some(AGENTS_GROUP_NAME.to_owned()),
                    ..TabGroup::new()
                };
                let group_id = group.id;
                self.tab_groups.insert(group_id, group);
                (group_id, self.pinned_boundary_index(&self.tabs))
            }
        };
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        if tab.group_id != Some(group_id) {
            tab.group_id = Some(group_id);
            self.move_tab_to_index(index, target, ctx);
        }
        self.expand_tab_group(group_id, ctx);
        ctx.dispatch_global_action("workspace:save_app", ());
        ctx.notify();
    }
}

#[cfg(test)]
#[path = "agent_session_tab_tests.rs"]
mod tests;
