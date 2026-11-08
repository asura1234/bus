//! Agent deletion and owned terminal identity checks.
use super::{
    deletion_identity, mpsc, schema, AgentId, BusEvent, LeftOpenReason, LeftOpenTerminal, Method,
    Provider, ResponseResult, RoomId, Worker,
};
use crate::messaging::identity;

/// Room-level launch request; provider preparation receives only LaunchSpec.
#[derive(Clone, Debug)]
pub(crate) struct AddAgent {
    pub(crate) room: RoomId,
    pub(crate) name: String,
    pub(crate) provider: Provider,
    pub(crate) cwd: String,
    pub(crate) extra_args: String,
    pub(crate) consent_project_hooks: bool,
}

pub(super) use super::first_message::first_message_text;

impl Worker {
    pub(super) fn delete_agent(
        &mut self,
        id: AgentId,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let was_pending = self
            .state
            .agent(id)
            .is_some_and(|agent| agent.deletion_pending);
        let mut state = self.state.clone();
        state.prepare_delete_agent(id).map_err(|e| e.to_string())?;
        self.save(state)?;
        if !was_pending {
            tracing::info!(
                event = "bus.deletion.pending",
                agent_id = id.0,
                "Agent deletion is durable; terminal close and removal are pending"
            );
        }
        let left_open: Vec<_> = self.stop_agent_terminal(id)?.into_iter().collect();
        let mut state = self.state.clone();
        state
            .delete_agent(id, super::io::now_ms())
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        tracing::info!(
            event = "bus.deletion.completed",
            agent_id = id.0,
            terminals_left_open = left_open.len(),
            "Agent removal is durable"
        );
        self.branch_checks.remove(&id);
        self.codex_rollouts.remove(&id);
        self.finish_deletion(left_open, events);
        Ok(())
    }

    pub(super) fn delete_room(
        &mut self,
        id: RoomId,
        events: &mpsc::Sender<BusEvent>,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        state.prepare_delete_room(id).map_err(|e| e.to_string())?;
        self.save(state)?;
        // The room's orchestrator in MASTER goes with it: it exists only for
        // this room, so its terminal is stopped like a member's.
        let agents = self.state.agents_deleted_with_room(id);
        let mut left_open = Vec::new();
        for agent in &agents {
            left_open.extend(self.stop_agent_terminal(*agent)?);
        }
        let mut state = self.state.clone();
        state
            .delete_room(id, super::io::now_ms())
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        for agent in agents {
            self.branch_checks.remove(&agent);
            self.codex_rollouts.remove(&agent);
        }
        self.finish_deletion(left_open, events);
        Ok(())
    }

    /// A completed deletion clears the previous failure. A terminal Bus no
    /// longer owns stays open and is reported rather than silently forgotten.
    fn finish_deletion(
        &mut self,
        left_open: Vec<LeftOpenTerminal>,
        events: &mpsc::Sender<BusEvent>,
    ) {
        self.error = None;
        for terminal in &left_open {
            tracing::warn!(event = "bus.deletion.terminal_left_open",
                agent_id = terminal.agent_id.0, agent_name = %terminal.agent_name,
                pane_id = %terminal.pane_id, terminal_id = %terminal.terminal_id,
                reason = ?terminal.reason, message = %terminal.message,
                "Deleted agent's terminal was left open");
        }
        if !left_open.is_empty() {
            let _ = events.send(BusEvent::TerminalsLeftOpen(left_open));
        }
    }

    /// Returns the terminal that was left open because the native server no
    /// longer attributes any terminal to this agent's managed name.
    fn stop_agent_terminal(&mut self, id: AgentId) -> Result<Option<LeftOpenTerminal>, String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        let identity = &agent.runtime_identity;
        let (Some(pane), Some(terminal)) = (&identity.pane_id, &identity.terminal_id) else {
            if deletion_identity::has_no_terminal_to_close(identity) {
                return Ok(None);
            }
            if deletion_identity::needs_native_absence_check(identity) {
                let name = identity::managed_name(agent.room_id, id);
                if self.native_ownership_released(&name) {
                    return Ok(None);
                }
            }
            return self.agent_error(id, "Cannot verify the terminal created by this launch. Deletion is suspended; inspect the launch outcome before retrying.".into()).map(|()| None);
        };
        if deletion_identity::terminal_already_closed(agent, terminal) {
            // Bus already closed it after saving the exited provider's screen.
            return Ok(None);
        }
        let managed_name = identity::managed_name(agent.room_id, id);
        let kind = identity::provider_kind(agent.provider).label();
        let expected_session = identity.session_id.clone();
        let left_open = LeftOpenTerminal::new(
            id,
            agent.name.clone(),
            pane.clone(),
            terminal.clone(),
            LeftOpenReason::SessionMoved,
        );
        let result = self.transport.request(Method::PaneCloseIfIdentity(
            schema::PaneCloseIfIdentityParams {
                pane_id: pane.clone(),
                expected_terminal_id: terminal.clone(),
                expected_agent: kind.into(),
                expected_managed_name: managed_name.clone(),
                expected_session_id: expected_session.clone(),
                close_exited_shell: false,
                allow_busy_shell: false,
            },
        ));
        self.settle_guarded_terminal_close(
            id,
            left_open,
            kind,
            &managed_name,
            expected_session,
            result,
        )
    }

    /// Settle a guarded pane close: confirmed, unconfirmed, or identity-changed.
    fn settle_guarded_terminal_close(
        &mut self,
        id: AgentId,
        left_open: LeftOpenTerminal,
        kind: &str,
        managed_name: &str,
        expected_session: Option<String>,
        result: Result<ResponseResult, crate::messaging::native::TransportError>,
    ) -> Result<Option<LeftOpenTerminal>, String> {
        match result {
            Ok(ResponseResult::Ok {}) => Ok(None),
            Ok(other) => {
                if super::super::diagnostics::dev_enabled() {
                    tracing::debug!(event = "bus.deletion.close_unconfirmed", agent_id = id.0,
                        response = ?other, "Unexpected guarded terminal close response");
                }
                self.agent_error(id, "Terminal close could not be confirmed. The agent and messages were kept. Retry deletion after checking its terminal.".into()).map(|()| None)
            }
            Err(error) => {
                if super::super::diagnostics::dev_enabled() {
                    tracing::debug!(event = "bus.deletion.close_failed", agent_id = id.0,
                        code = ?error.code, detail = %error.message, "Guarded terminal close failed");
                }
                if error.code.as_deref() == Some("terminal_identity_changed") {
                    if self.reconcile_deleting_initial_session(id)? {
                        // The first session binding is now durable. Retry the exact
                        // same native guard; a different/rebound session still fails.
                        return self.stop_agent_terminal(id);
                    }
                    if self.native_ownership_released(&managed_name) {
                        // The native server cleared this launch's managed name, e.g.
                        // after the provider exited and the pane fell back to its
                        // shell. That idle shell in Bus's own terminal is closed;
                        // anything running in it now is not ours, so the server
                        // refuses and the terminal is left open.
                        return Ok(self.close_exited_shell(
                            left_open,
                            kind,
                            &managed_name,
                            expected_session,
                        ));
                    }
                    if expected_session.as_deref().is_some_and(|expected| {
                        self.owned_terminal_session_moved(&left_open, kind, &managed_name, expected)
                    }) {
                        // The provider session in this agent's own terminal
                        // moved on, as after an agent clear the server did not
                        // record or a chat resumed by hand. No retry can pass
                        // the guard, and the session there may not be Bus's to
                        // end, so the agent goes and the terminal stays open.
                        tracing::info!(event = "bus.deletion.session_moved", agent_id = id.0,
                            pane_id = %left_open.pane_id, terminal_id = %left_open.terminal_id,
                            "Owned terminal now runs another provider session; left open");
                        return Ok(Some(left_open));
                    }
                    if let Some(moved) = expected_session.as_deref().and_then(|expected| {
                        self.restored_terminal_session_moved(
                            &left_open,
                            kind,
                            &managed_name,
                            expected,
                        )
                    }) {
                        // After a restart the managed name moved to a restored
                        // terminal that kept another provider session, so Bus
                        // never rebound to it. Same reasoning as above.
                        tracing::info!(event = "bus.deletion.session_moved", agent_id = id.0,
                            pane_id = %moved.pane_id, terminal_id = %moved.terminal_id,
                            "Restored terminal runs another provider session; left open");
                        return Ok(Some(moved));
                    }
                }
                let message = if matches!(
                    error.code.as_deref(),
                    Some("unknown_method" | "invalid_request")
                ) {
                    "Update and restart the Bus server, then retry deletion. The agent and messages were kept.".into()
                } else if error.code.as_deref() == Some("terminal_identity_changed") {
                    "The terminal session changed. The agent and messages were kept. Check its terminal before retrying deletion.".into()
                } else {
                    "The terminal could not be closed. The agent and messages were kept. Check its terminal, then retry deletion.".into()
                };
                self.agent_error(id, message).map(|()| None)
            }
        }
    }

    /// Whether the native server still shows this agent's own terminal, pane,
    /// managed name and provider, with a provider session other than `expected`.
    /// A failed lookup proves nothing.
    fn owned_terminal_session_moved(
        &mut self,
        terminal: &LeftOpenTerminal,
        kind: &str,
        managed_name: &str,
        expected: &str,
    ) -> bool {
        let Ok(ResponseResult::AgentInfo { agent: info }) =
            self.transport
                .request(Method::AgentGet(schema::AgentTarget {
                    target: terminal.pane_id.clone(),
                }))
        else {
            return false;
        };
        info.terminal_id == terminal.terminal_id
            && info.pane_id == terminal.pane_id
            && info.name.as_deref() == Some(managed_name)
            && info.agent.as_deref() == Some(kind)
            && info.agent_session.is_some_and(|session| {
                session.source == format!("herdr:{kind}") && session.value != expected
            })
    }

    /// The one native terminal that carries this agent's managed name and
    /// provider when it is not the terminal Bus recorded and runs a provider
    /// session other than `expected`. A failed lookup proves nothing.
    fn restored_terminal_session_moved(
        &mut self,
        recorded: &LeftOpenTerminal,
        kind: &str,
        managed_name: &str,
        expected: &str,
    ) -> Option<LeftOpenTerminal> {
        let Ok(ResponseResult::AgentList { agents }) = self
            .transport
            .request(Method::AgentList(schema::EmptyParams {}))
        else {
            return None;
        };
        let mut named = agents
            .into_iter()
            .filter(|info| info.name.as_deref() == Some(managed_name));
        let info = named.next()?;
        if named.next().is_some()
            || info.terminal_id == recorded.terminal_id
            || info.agent.as_deref() != Some(kind)
            || !info.agent_session.as_ref().is_some_and(|session| {
                session.source == format!("herdr:{kind}") && session.value != expected
            })
        {
            return None;
        }
        Some(LeftOpenTerminal::new(
            recorded.agent_id,
            recorded.agent_name.clone(),
            info.pane_id,
            info.terminal_id,
            LeftOpenReason::SessionMoved,
        ))
    }

    /// Close the agent's own terminal once its provider exited, even with a
    /// command running in the shell left behind (the server requires its own
    /// exit evidence for that); else return it as left open: the server
    /// refused, for example because another agent or managed name took it.
    fn close_exited_shell(
        &mut self,
        terminal: LeftOpenTerminal,
        kind: &str,
        managed_name: &str,
        session: Option<String>,
    ) -> Option<LeftOpenTerminal> {
        let closed = self
            .request_exited_shell_close(
                &terminal.pane_id,
                &terminal.terminal_id,
                kind,
                managed_name,
                session,
            )
            .is_ok();
        if closed {
            tracing::info!(event = "bus.deletion.exited_shell_closed", agent_id = terminal.agent_id.0,
                pane_id = %terminal.pane_id, terminal_id = %terminal.terminal_id,
                "Closed the shell the exited provider left");
            return None;
        }
        tracing::info!(event = "bus.deletion.terminal_released", agent_id = terminal.agent_id.0,
            pane_id = %terminal.pane_id, terminal_id = %terminal.terminal_id,
            "Native terminal no longer carries the Bus managed name; left open");
        Some(LeftOpenTerminal::new(
            terminal.agent_id,
            terminal.agent_name,
            terminal.pane_id,
            terminal.terminal_id,
            LeftOpenReason::ProviderExited,
        ))
    }

    /// The guarded close of the shell an exited provider left in this exact
    /// terminal, busy or idle; the server keeps it unless it saw that exit.
    pub(super) fn request_exited_shell_close(
        &mut self,
        pane: &str,
        terminal: &str,
        kind: &str,
        managed_name: &str,
        session: Option<String>,
    ) -> Result<(), crate::messaging::native::TransportError> {
        match self.transport.request(Method::PaneCloseIfIdentity(
            schema::PaneCloseIfIdentityParams {
                pane_id: pane.into(),
                expected_terminal_id: terminal.into(),
                expected_agent: kind.into(),
                expected_managed_name: managed_name.into(),
                expected_session_id: session,
                close_exited_shell: true,
                allow_busy_shell: true,
            },
        )) {
            Ok(ResponseResult::Ok {}) => Ok(()),
            Ok(other) => Err(crate::messaging::native::TransportError {
                code: None,
                message: format!("Unexpected close response: {other:?}"),
                definitely_rejected: false,
            }),
            Err(error) => Err(error),
        }
    }

    /// `agent.list` includes every terminal carrying a managed name, even with
    /// no detected provider, so absence is a native fact. A failed lookup
    /// proves nothing and keeps deletion suspended.
    fn native_ownership_released(&mut self, managed_name: &str) -> bool {
        matches!(
            self.transport.request(Method::AgentList(schema::EmptyParams {})),
            Ok(ResponseResult::AgentList { agents })
                if !agents.iter().any(|info| info.name.as_deref() == Some(managed_name))
        )
    }

    pub(super) fn agent_error(&mut self, id: AgentId, message: String) -> Result<(), String> {
        let mut state = self.state.clone();
        state
            .set_agent_error(id, Some(message.clone()))
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Err(message)
    }
}
