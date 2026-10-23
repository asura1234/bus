//! Read only a failed launch's own pane; do not infer an outage from generic exit text.
use super::{schema, BusState, Method, ResponseResult, RoomAgent, RoomId, RuntimeStatus, Worker};
use crate::agents::providers::launch;

impl Worker {
    pub(super) fn ensure_provider_binary(
        &mut self,
        room: RoomId,
        provider: super::Provider,
    ) -> Result<(), String> {
        if launch::binary_available(provider) {
            return Ok(());
        }
        let binary = launch::executable(provider);
        let mut state = self.state.clone();
        let error = state
            .report_missing_binary(room, binary, super::io::now_ms())
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Err(error)
    }

    pub(super) fn observe_missing_binary(
        &mut self,
        state: &mut BusState,
        agent: &RoomAgent,
        status: RuntimeStatus,
        info: Option<&schema::AgentInfo>,
    ) -> Result<(bool, Option<bool>), String> {
        if info.is_some_and(|i| i.interactive_ready) {
            self.missing_binary_checked.remove(&agent.id);
            state
                .clear_missing_binary(agent.id, super::io::now_ms())
                .map_err(|e| e.to_string())?;
            return Ok((false, None));
        }
        if agent.missing_binary.is_some() {
            return Ok((true, None));
        }
        if status != RuntimeStatus::Unavailable {
            self.missing_binary_checked.remove(&agent.id);
            return Ok((false, None));
        }
        let terminal = agent.runtime_identity.terminal_id.clone();
        let Some(pane) = agent.runtime_identity.pane_id.as_deref() else {
            return Ok((false, None));
        };
        let evidence = self.transport.request(Method::PaneGet(schema::PaneTarget {
            pane_id: pane.into(),
        }));
        let Ok(ResponseResult::PaneInfo { pane: evidence }) = evidence else {
            return Ok((false, None));
        };
        if evidence.pane_id != pane || Some(&evidence.terminal_id) != terminal.as_ref() {
            return Ok((false, None));
        }
        if evidence.agent_process_exited {
            return Ok((false, Some(true)));
        }
        if self.missing_binary_checked.get(&agent.id) == Some(&terminal) {
            return Ok((false, Some(false)));
        }
        let Ok(screen) = self.read_verified_startup_screen(agent.id) else {
            // A failed read is not evidence; retry once native identity is readable.
            return Ok((false, Some(false)));
        };
        let binary = launch::executable(agent.provider);
        if !missing_binary_screen(&screen, binary) {
            self.missing_binary_checked.insert(agent.id, terminal);
            return Ok((false, Some(false)));
        }
        tracing::warn!(
            event = "bus.provider.binary_missing",
            agent_id = agent.id.0,
            room_id = agent.room_id.0,
            binary,
            "Provider launch could not find its binary"
        );
        state
            .block_missing_binary(agent.id, binary, super::io::now_ms())
            .map_err(|e| e.to_string())?;
        Ok((true, Some(false)))
    }
}

fn missing_binary_screen(screen: &str, binary: &str) -> bool {
    screen.lines().any(|line| {
        let line = line.strip_prefix("> ").unwrap_or(line).trim();
        line == format!("zsh: command not found: {binary}")
            || line == format!("fish: Unknown command: {binary}")
            || line == format!("{binary}: command not found")
            || line.ends_with(&format!(": {binary}: command not found"))
            || line.ends_with(&format!(": {binary}: not found"))
            || line.starts_with(&format!(
                "'{binary}' is not recognized as an internal or external command"
            ))
    })
}
