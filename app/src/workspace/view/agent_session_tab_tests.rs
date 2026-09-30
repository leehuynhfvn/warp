use warp_core::features::FeatureFlag;
use warpui::App;

use super::AGENTS_GROUP_NAME;
use crate::workspace::view::tests::{initialize_app, mock_workspace};

#[test]
fn the_tab_opens_beside_the_users_without_taking_their_place() {
    let _flag = FeatureFlag::GroupedTabs.override_enabled(false);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);
        let (tabs_before, active_before) = workspace.read(&app, |workspace, _| {
            (workspace.tab_count(), workspace.active_tab_index())
        });

        let opened = workspace
            .update(&mut app, |workspace, ctx| {
                workspace.open_agent_session_tab("lab-1", ctx)
            })
            .expect("the tab opens");

        workspace.read(&app, |workspace, ctx| {
            assert_eq!(workspace.tab_count(), tabs_before + 1);
            assert_eq!(
                workspace.active_tab_index(),
                active_before,
                "the user stays in the tab they were in"
            );
            let new_tab = &workspace.tabs[active_before + 1];
            assert_eq!(
                new_tab.pane_group.as_ref(ctx).display_title(ctx),
                "Agent · lab-1"
            );
            assert!(new_tab.group_id.is_none(), "no tab group without the flag");
            assert!(workspace.tab_groups.is_empty());
            assert!(
                new_tab
                    .pane_group
                    .as_ref(ctx)
                    .terminal_view_from_pane_id(opened.pane_id, ctx)
                    .is_some(),
                "the pane that was returned belongs to the new tab"
            );
        });
    });
}

#[test]
fn tabs_of_agents_go_into_one_group_that_is_shown_expanded() {
    let _flag = FeatureFlag::GroupedTabs.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);

        for alias in ["lab-1", "lab-2"] {
            workspace
                .update(&mut app, |workspace, ctx| {
                    workspace.open_agent_session_tab(alias, ctx)
                })
                .expect("the tab opens");
            workspace.update(&mut app, |workspace, _| {
                for group in workspace.tab_groups.values_mut() {
                    group.collapsed = true;
                }
            });
        }
        workspace
            .update(&mut app, |workspace, ctx| {
                workspace.open_agent_session_tab("lab-3", ctx)
            })
            .expect("the tab opens");

        workspace.read(&app, |workspace, ctx| {
            assert_eq!(workspace.tab_groups.len(), 1, "one group is reused");
            let group = workspace.tab_groups.values().next().unwrap();
            assert_eq!(group.name.as_deref(), Some(AGENTS_GROUP_NAME));
            assert!(!group.collapsed, "opening a session shows the group");

            let members: Vec<usize> = workspace
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, tab)| tab.group_id == Some(group.id))
                .map(|(index, _)| index)
                .collect();
            assert_eq!(members.len(), 3);
            assert!(
                members.windows(2).all(|pair| pair[1] == pair[0] + 1),
                "the group stays in one run: {members:?}"
            );
            let titles: Vec<String> = members
                .iter()
                .map(|index| {
                    workspace.tabs[*index]
                        .pane_group
                        .as_ref(ctx)
                        .display_title(ctx)
                })
                .collect();
            assert_eq!(titles, ["Agent · lab-1", "Agent · lab-2", "Agent · lab-3"]);
        });
    });
}

#[test]
fn the_group_does_not_swallow_the_tab_the_user_was_in() {
    let _flag = FeatureFlag::GroupedTabs.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);
        let user_tab = workspace.read(&app, |workspace, _| workspace.tabs[0].pane_group.id());

        workspace
            .update(&mut app, |workspace, ctx| {
                workspace.open_agent_session_tab("lab-1", ctx)
            })
            .expect("the tab opens");

        workspace.read(&app, |workspace, _| {
            let active = &workspace.tabs[workspace.active_tab_index()];
            assert_eq!(active.pane_group.id(), user_tab);
            assert!(active.group_id.is_none());
        });
    });
}

#[test]
fn an_alias_that_could_change_the_ssh_command_opens_nothing() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let workspace = mock_workspace(&mut app);
        let tabs_before = workspace.read(&app, |workspace, _| workspace.tab_count());

        for alias in ["-oProxyCommand=x", "lab-1; reboot", "lab 1", ""] {
            let result = workspace.update(&mut app, |workspace, ctx| {
                workspace.open_agent_session_tab(alias, ctx)
            });
            assert!(result.is_err(), "{alias:?}");
        }

        workspace.read(&app, |workspace, _| {
            assert_eq!(workspace.tab_count(), tabs_before);
        });
    });
}
