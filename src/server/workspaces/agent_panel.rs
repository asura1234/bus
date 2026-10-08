use crate::app::AppState;
use crate::detect::AgentState;
use crate::terminal::TerminalRuntimeRegistry;

pub(crate) struct AgentPanelEntry {
    pub ws_idx: usize,
    pub pane_id: crate::layout::PaneId,
    pub state: AgentState,
    pub seen: bool,
    pub last_agent_state_change_seq: Option<u64>,
}

pub(crate) fn agent_panel_entries_from(
    app: &AppState,
    _terminal_runtimes: &TerminalRuntimeRegistry,
) -> Vec<AgentPanelEntry> {
    let mut entries: Vec<AgentPanelEntry> = app
        .workspaces
        .iter()
        .enumerate()
        .flat_map(|(ws_idx, workspace)| {
            workspace
                .pane_details(&app.terminals)
                .into_iter()
                .map(move |detail| AgentPanelEntry {
                    ws_idx,
                    pane_id: detail.pane_id,
                    state: detail.state,
                    seen: detail.seen,
                    last_agent_state_change_seq: detail.last_agent_state_change_seq,
                })
        })
        .collect();
    super::agent_view::apply_agent_view(app, &mut entries);
    entries
}
