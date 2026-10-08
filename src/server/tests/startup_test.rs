use super::*;

#[test]
fn startup_uses_configured_agent_panel_sort() {
    let mut config = Config::default();
    config.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

    let app = App::new(
        &config,
        crate::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::api::EventHub::default(),
    );

    assert_eq!(app.state.agent_panel_sort, state::AgentPanelSort::Priority);
    assert!(app.state.workspaces.is_empty());
    assert!(app.state.terminals.is_empty());
    assert_eq!(app.terminal_runtimes.len(), 0);
}
