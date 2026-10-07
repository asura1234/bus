use crate::ui::AgentPanelEntry;

use super::AppState;

pub(crate) fn apply_agent_view(app: &AppState, entries: &mut [AgentPanelEntry]) {
    if matches!(
        app.agent_panel_sort,
        crate::app::state::AgentPanelSort::Priority
    ) {
        entries.sort_by_key(|entry| {
            (
                std::cmp::Reverse(super::api_helpers::tab_attention_priority(
                    entry.state,
                    entry.seen,
                )),
                std::cmp::Reverse(entry.last_agent_state_change_seq),
            )
        });
    }
}

pub(crate) fn presented_workspace_idx(app: &AppState) -> Option<usize> {
    app.active
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AgentPanelSort;
    use crate::detect::{Agent, AgentState};
    use crate::workspace::Workspace;

    fn state_with_agents() -> AppState {
        let mut state = AppState::test_new();
        state.workspaces = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        state.ensure_test_terminals();
        state.active = Some(0);
        state.selected = 0;
        for (ws_idx, agent_state) in [(0, AgentState::Idle), (1, AgentState::Working)] {
            let pane_id = state.workspaces[ws_idx].tabs[0].root_pane;
            let terminal_id = state.workspaces[ws_idx].tabs[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            let terminal = state.terminals.get_mut(&terminal_id).unwrap();
            terminal.detected_agent = Some(Agent::Claude);
            terminal.state = agent_state;
        }
        state
    }

    fn projected_entries(state: &AppState) -> Vec<crate::ui::AgentPanelEntry> {
        crate::ui::agent_panel_entries_from(state, &crate::terminal::TerminalRuntimeRegistry::new())
    }

    #[test]
    fn space_order_keeps_entries_from_all_workspaces() {
        let state = state_with_agents();
        let entries = projected_entries(&state);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].ws_idx, 0);
        assert_eq!(entries[1].ws_idx, 1);
    }

    #[test]
    fn priority_order_keeps_the_agent_requiring_attention_first() {
        let mut state = state_with_agents();
        state.agent_panel_sort = AgentPanelSort::Priority;
        for (ws_idx, agent_state) in [(0, AgentState::Working), (1, AgentState::Blocked)] {
            let pane = state.workspaces[ws_idx].tabs[0].root_pane;
            let terminal = state.workspaces[ws_idx].tabs[0]
                .terminal_id(pane)
                .unwrap()
                .clone();
            state.terminals.get_mut(&terminal).unwrap().state = agent_state;
        }
        let entries = projected_entries(&state);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].ws_idx, 1);
        assert_eq!(entries[1].ws_idx, 0);
    }

    #[test]
    fn equal_priority_entries_show_the_latest_state_change_first() {
        let mut state = state_with_agents();
        state.agent_panel_sort = AgentPanelSort::Priority;
        for (ws_idx, sequence) in [(0, 1), (1, 3)] {
            let pane = state.workspaces[ws_idx].tabs[0].root_pane;
            let terminal = state.workspaces[ws_idx].tabs[0]
                .terminal_id(pane)
                .unwrap()
                .clone();
            let terminal = state.terminals.get_mut(&terminal).unwrap();
            terminal.state = AgentState::Working;
            terminal.last_agent_state_change_seq = Some(sequence);
        }
        let entries = projected_entries(&state);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].ws_idx, 1);
        assert_eq!(entries[1].ws_idx, 0);
    }

    #[test]
    fn presented_workspace_tracks_the_active_workspace() {
        let mut state = state_with_agents();
        assert_eq!(presented_workspace_idx(&state), Some(0));
        state.active = Some(1);
        assert_eq!(presented_workspace_idx(&state), Some(1));
        state.active = None;
        assert_eq!(presented_workspace_idx(&state), None);
    }
}
