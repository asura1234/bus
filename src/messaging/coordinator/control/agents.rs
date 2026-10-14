//! Agents development commands on the single coordinator.
use super::{
    launch, mpsc, optional_bool, optional_text, required, schema, unique, AddAgent, AgentId,
    BusCommand, BusEvent, Method, Provider, ResponseResult, RoomId, RuntimeStatus, Worker,
    MASTER_AGENT_NEEDS_ROOM,
};
use crate::messaging::orchestration::OrchestratorSpec;
use serde_json::{json, Value};

impl Worker {
    pub(super) fn execute_agents(
        &mut self,
        method: &str,
        p: &Value,
        _events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<Value, String> {
        match method {
            "agent.details" => self.control_command(BusCommand::SetDetails(
                self.control_agent(required(p, "agent")?, None)?,
                p.get("on")
                    .and_then(Value::as_bool)
                    .ok_or("Details must be on or off")?,
            )),
            "agent.rename" => self.control_command(BusCommand::RenameAgent(
                self.control_agent(required(p, "agent")?, None)?,
                required(p, "name")?.into(),
            )),
            "agent.delete" => self.control_command(BusCommand::DeleteAgent(
                self.control_agent(required(p, "agent")?, None)?,
            )),
            "agent.setup-confirm" => self.control_command(BusCommand::CompleteHookSetup(
                self.control_agent(required(p, "agent")?, None)?,
            )),
            "agent.add" => {
                let provider = match required(p, "provider")? {
                    "claude" => Provider::ClaudeCode,
                    "codex" => Provider::Codex,
                    "cursor" => Provider::Cursor,
                    _ => return Err("Provider must be claude, codex, or cursor".into()),
                };
                let room = self.control_room(required(p, "room")?)?;
                let master = self.state.master_room().is_some_and(|m| m.id == room);
                let orchestrates = optional_text(p, "orchestrates")?;
                let system_prompt = optional_text(p, "system_prompt")?;
                if !master && system_prompt.is_some() {
                    return Err("--system-prompt applies only to MASTER orchestrators".into());
                }
                let input = AddAgent {
                    room,
                    name: required(p, "name")?.into(),
                    provider,
                    cwd: required(p, "cwd")?.into(),
                    extra_args: optional_text(p, "extra_args")?.unwrap_or_default().into(),
                    consent_project_hooks: optional_bool(p, "consent_project_hooks")?,
                };
                // Every MASTER agent is an orchestrator of exactly one work
                // room; outside MASTER, --orchestrates still reaches the
                // model's MASTER-only check.
                self.control_command(match orchestrates {
                    Some(room) => BusCommand::AddOrchestrator(
                        input,
                        OrchestratorSpec {
                            room: self.control_room(room)?,
                            system_prompt: system_prompt.map(Into::into),
                        },
                    ),
                    None if master => return Err(MASTER_AGENT_NEEDS_ROOM.into()),
                    None => BusCommand::AddAgent(input),
                })
            }
            "agent.clear" => {
                let agent = self.control_agent(required(p, "agent")?, None)?;
                self.control_clear(agent)
            }
            "agent.interrupt" => self.control_interrupt(p),
            _ => Err("Unknown method".into()),
        }
    }

    pub(in crate::messaging::coordinator) fn control_agent(
        &self,
        selector: &str,
        room: Option<RoomId>,
    ) -> Result<AgentId, String> {
        unique(
            self.state
                .agents()
                .filter(|a| {
                    room.is_none_or(|r| r == a.room_id)
                        && (selector == a.id.0.to_string() || selector == a.name)
                })
                .map(|a| a.id),
        )
    }

    /// Starts a fresh provider context in an idle agent's terminal. The reset
    /// command is typed directly, not sent as a Bus message, and the agent
    /// rebinds to the new provider session its next callback reports.
    pub(in crate::messaging::coordinator) fn control_clear(
        &mut self,
        id: AgentId,
    ) -> Result<Value, String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        if agent.deletion_pending
            || agent.session_binding_invalidated
            || !agent.hook_setup_confirmed
            || agent.status != RuntimeStatus::Idle
        {
            return Err("Only an idle, ready agent can be cleared".into());
        }
        if agent.current_request.is_some() || self.state.next_queued_request(id).is_some() {
            return Err(
                "The agent still has messages to answer; clear it once they are done".into(),
            );
        }
        let identity = &agent.runtime_identity;
        let (Some(terminal), Some(pane)) = (&identity.terminal_id, &identity.pane_id) else {
            return Err("The agent has no terminal yet".into());
        };
        let text = clear_command(agent.provider).to_string();
        let method = match &identity.session_id {
            Some(session) => Method::AgentPromptIfIdle(schema::AgentPromptIfIdleParams {
                target: pane.clone(),
                text: text.clone(),
                expected_terminal_id: terminal.clone(),
                expected_pane_id: pane.clone(),
                expected_agent: launch::provider_kind(agent.provider).into(),
                expected_session_id: session.clone(),
                steer: false,
            }),
            // Codex starts its provider session with the first turn.
            None if agent.provider == Provider::Codex => {
                Method::AgentPromptIfUnbound(schema::AgentPromptIfUnboundParams {
                    target: pane.clone(),
                    text: text.clone(),
                    expected_terminal_id: terminal.clone(),
                    expected_pane_id: pane.clone(),
                    expected_managed_name: format!("bus-r{}-a{}", agent.room_id.0, agent.id.0),
                })
            }
            None => return Err("The agent has no provider session yet".into()),
        };
        // The reset intent is durable before the provider can start a fresh
        // session, so that session's callbacks rebind instead of being rejected.
        let before = self.state.clone();
        let mut state = before.clone();
        state.begin_session_reset(id).map_err(|e| e.to_string())?;
        self.save(state)?;
        match self.transport.request(method) {
            Ok(ResponseResult::AgentPrompted { .. }) => {}
            Ok(other) => return Err(format!("Unexpected response to {text}: {other:?}")),
            // Only a definite rejection proves no reset was typed; an
            // uncertain one keeps the intent for a session that may follow.
            Err(error) if error.definitely_rejected => {
                self.save(before)?;
                return Err(error.message);
            }
            Err(error) => return Err(error.message),
        }
        Ok(json!({"agent_id": id, "sent": text, "stage": "cleared"}))
    }
}

impl Worker {
    /// Presses the provider's stop key in one agent's terminal, or in every
    /// working or blocked agent's of a room but the author's. Nothing is
    /// killed and the context stays. The turn closes as interrupted when Bus
    /// next sees the agent idle; the CLI waits for that to report the stop.
    pub(in crate::messaging::coordinator) fn control_interrupt(
        &mut self,
        p: &Value,
    ) -> Result<Value, String> {
        let (room, agent) = match (optional_text(p, "agent")?, optional_text(p, "room")?) {
            (Some(agent), None) => {
                let id = self.control_agent(agent, None)?;
                (
                    self.state.agent(id).ok_or("Unknown agent")?.room_id,
                    Some(id),
                )
            }
            (None, Some(room)) => (self.control_room(room)?, None),
            _ => return Err("Name one agent, or a room with --room".into()),
        };
        let author = optional_text(p, "as")?
            .map(|selector| self.control_author(room, selector))
            .transpose()?;
        let targets: Vec<AgentId> = match agent {
            Some(id) if author == Some(id) => {
                return Err("An agent cannot interrupt itself".into());
            }
            Some(id) => vec![id],
            None => self
                .state
                .agents()
                .filter(|a| a.room_id == room && Some(a.id) != author && !a.deletion_pending)
                .filter(|a| {
                    matches!(
                        a.shown_status(),
                        RuntimeStatus::Working | RuntimeStatus::Blocked
                    )
                })
                .map(|a| a.id)
                .collect(),
        };
        let now = crate::messaging::storage::io::now_ms();
        let interrupter = author.map_or(
            crate::messaging::model::Author::Developer,
            crate::messaging::model::Author::Agent,
        );
        let mut state = self.state.clone();
        let mut rows = Vec::new();
        for id in targets {
            let agent = state.agent(id).ok_or("Unknown agent")?;
            let mut row = json!({"agent_id": id, "name": agent.name});
            let status = agent.shown_status();
            match status {
                RuntimeStatus::Idle => {
                    // Nothing to stop. A turn Bus still holds (settling, or
                    // waiting on background shells) and the queue close.
                    let closed = state
                        .close_interrupted(id, now)
                        .map_err(|e| e.to_string())?;
                    // Their waiters point to the history for who stopped them.
                    if !closed.is_empty() {
                        state
                            .post_interrupt_notice(room, interrupter.clone(), id, now)
                            .map_err(|e| e.to_string())?;
                    }
                    row["outcome"] = json!("already_idle");
                }
                RuntimeStatus::Working | RuntimeStatus::Blocked => {
                    let key = interrupt_key(agent.provider);
                    let Some(pane) = agent.runtime_identity.pane_id.clone() else {
                        row["outcome"] = json!("failed");
                        row["reason"] = json!("The agent has no terminal yet");
                        rows.push(row);
                        continue;
                    };
                    // One press only: a second Esc opens Claude's rewind or
                    // Codex's edit-previous-message, a second Ctrl+C quits Cursor.
                    let sent = self.transport.request(Method::AgentSendKeys(
                        schema::AgentSendKeysParams {
                            target: pane,
                            keys: vec![key.into()],
                        },
                    ));
                    match sent {
                        Err(error) if error.definitely_rejected => {
                            tracing::warn!(
                                event = "bus.agent.interrupt",
                                agent_id = id.0,
                                key,
                                error = %error.message,
                                "Stop key refused"
                            );
                            row["outcome"] = json!("failed");
                            row["reason"] = json!(error.message);
                        }
                        // An uncertain failure may have pressed the key; the
                        // CLI's wait for idle decides.
                        _ => {
                            tracing::info!(
                                event = "bus.agent.interrupt",
                                agent_id = id.0,
                                key,
                                "Stop key pressed"
                            );
                            state.begin_interrupt(id).map_err(|e| e.to_string())?;
                            state
                                .post_interrupt_notice(room, interrupter.clone(), id, now)
                                .map_err(|e| e.to_string())?;
                            row["outcome"] = json!("interrupting");
                            row["key"] = json!(key);
                        }
                    }
                }
                other => {
                    row["outcome"] = json!("failed");
                    row["reason"] = json!(format!(
                        "agent {} is {}; there is no turn to interrupt",
                        agent.name,
                        json!(other).as_str().unwrap_or("not running")
                    ));
                }
            }
            rows.push(row);
        }
        self.save(state)?;
        Ok(json!({"room_id": room, "agents": rows}))
    }
}

/// The key that stops a running turn, from each provider's own working hint:
/// Claude Code and Codex show `esc to interrupt`, Cursor `ctrl+c to stop`.
fn interrupt_key(provider: Provider) -> &'static str {
    match provider {
        Provider::ClaudeCode | Provider::Codex => "esc",
        Provider::Cursor => "ctrl+c",
    }
}

/// The provider command that starts a fresh context in the same terminal.
/// Codex's `/new` asks where the new conversation runs; `/clear` does not.
fn clear_command(provider: Provider) -> &'static str {
    match provider {
        Provider::ClaudeCode | Provider::Codex => "/clear",
        Provider::Cursor => "/new-chat",
    }
}
