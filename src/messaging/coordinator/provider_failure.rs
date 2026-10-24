//! Read only a failed launch's own pane; do not infer an outage from generic exit text.
use super::{schema, BusState, Method, ResponseResult, RoomAgent, RoomId, RuntimeStatus, Worker};
use crate::agents::providers::launch;

impl Worker {
    pub(super) fn ensure_provider_binary(
        &self,
        room: RoomId,
        provider: super::Provider,
    ) -> Result<(), String> {
        if launch::binary_available(provider) {
            return Ok(());
        }
        let binary = launch::executable(provider);
        // Refusing a new agent has no retained work or saved agent to block.
        Err(BusState::missing_binary_message(room, binary))
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
                .clear_missing_binary(agent.id)
                .map_err(|e| e.to_string())?;
            return Ok((false, None));
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
        let evidence = match evidence {
            Ok(ResponseResult::PaneInfo { pane }) => pane,
            Err(error)
                if error.definitely_rejected && error.code.as_deref() == Some("pane_not_found") =>
            {
                // A close may have succeeded just before its state save
                // failed. Keep its address through the idempotent close pass
                // so that pass can durably record success and detach it.
                if !agent.exit_snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.close_pending()
                        && Some(&snapshot.terminal_id) == terminal.as_ref()
                        && snapshot.launch_id == agent.runtime_identity.launch_id
                }) {
                    clear_stale_pane(state, agent)?;
                }
                return Ok((false, None));
            }
            _ => return Ok((false, None)),
        };
        if evidence.pane_id != pane || Some(&evidence.terminal_id) != terminal.as_ref() {
            clear_stale_pane(state, agent)?;
            return Ok((false, None));
        }
        if agent.missing_binary.is_some() {
            return Ok((true, None));
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

fn clear_stale_pane(state: &mut BusState, agent: &RoomAgent) -> Result<(), String> {
    let mut identity = agent.runtime_identity.clone();
    identity.pane_id = None;
    state
        .set_agent_runtime_identity(agent.id, identity)
        .map_err(|e| e.to_string())?;
    if agent.actionable_error.is_none() {
        state.set_agent_error(agent.id, Some(format!(
            "Agent terminal is no longer available at its recorded pane. Saved conversation and queued work retained; inspect the agent, then run: bus agent resume --room {}", agent.room_id.0
        ))).map_err(|e| e.to_string())?;
    }
    tracing::warn!(event = "bus.agent.pane_detached", agent_id = agent.id.0,
        pane_id = ?agent.runtime_identity.pane_id, "Cleared a missing or replaced pane reference");
    Ok(())
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
