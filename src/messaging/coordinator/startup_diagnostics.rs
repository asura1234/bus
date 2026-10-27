//! Persist only startup evidence, never queued request bodies or conversation history.
use super::{schema, Method, ResponseResult, RoomAgent, Worker};

impl Worker {
    pub(super) fn startup_diagnostics(&mut self, agent: &RoomAgent) -> String {
        let identity = &agent.runtime_identity;
        let mut evidence = serde_json::json!({
            "expected": {
                "agent_id": agent.id.0,
                "managed_name": crate::messaging::identity::managed_name(agent.room_id, agent.id),
                "pane_id": identity.pane_id,
                "terminal_id": identity.terminal_id,
            }
        });
        let snapshot = if identity.session_id.is_some()
            || agent.current_request.is_some()
            || agent
                .startup
                .as_ref()
                .is_some_and(|startup| startup.native_terminal_id.is_some())
        {
            Err("Conversation/resume screen omitted to exclude message contents".into())
        } else if let (Some(pane), Some(terminal)) = (&identity.pane_id, &identity.terminal_id) {
            self.transport
                .request(Method::AgentStartupSnapshot(
                    schema::AgentStartupSnapshotParams {
                        pane_id: pane.clone(),
                        expected_terminal_id: terminal.clone(),
                    },
                ))
                .map_err(|error| error.message)
                .and_then(|result| match result {
                    ResponseResult::AgentStartupSnapshot { snapshot }
                        if snapshot.pane_id == *pane && snapshot.terminal_id == *terminal =>
                    {
                        Ok(snapshot)
                    }
                    _ => Err(
                        "Startup snapshot identity or response changed; no screen captured".into(),
                    ),
                })
        } else {
            Err("Launch pane/terminal identity is unavailable".into())
        };
        match snapshot {
            Ok(snapshot) => evidence["native"] = serde_json::json!(snapshot),
            Err(error) => evidence["unavailable"] = error.into(),
        }
        evidence.to_string()
    }
}
