//! Bind coordinator obligations to the native detector without duplicating composer parsing.
use crate::protocol::api::schema::AgentDetectionContexts;
use crate::server::app::App;

impl App {
    pub(in crate::server::api) fn handle_agent_list_with_detection_context(
        &mut self,
        id: String,
        params: AgentDetectionContexts,
    ) -> String {
        let mut updated = std::collections::HashSet::new();
        for context in params.agents {
            let Some((ws, pane)) = self.parse_current_public_pane_id(&context.pane_id) else {
                continue;
            };
            let Some(info) = self.agent_info(ws, pane) else {
                continue;
            };
            if info.terminal_id != context.terminal_id
                || info
                    .agent_session
                    .as_ref()
                    .map(|session| session.value.as_str())
                    != context.session_id.as_deref()
            {
                continue;
            }
            let Some(kind) = info
                .agent
                .as_deref()
                .and_then(crate::agents::parse_agent_label)
            else {
                continue;
            };
            if let Some(runtime) = self.lookup_runtime_sender(ws, pane) {
                runtime.set_detection_context(
                    kind,
                    context.obligation,
                    context.activity_age_ms,
                    super::prompt::detection_composer(kind),
                );
                updated.insert(context.terminal_id);
            }
        }
        for terminal in self.state.terminals.values() {
            if !updated.contains(terminal.id.as_str()) {
                if let Some(runtime) = self.terminal_runtimes.get(&terminal.id) {
                    runtime.clear_detection_context();
                }
            }
        }
        self.handle_agent_list(id)
    }
}
