//! Agents development commands on the single coordinator.
use super::{
    launch, mpsc, optional_bool, optional_text, required, schema, AddAgent, AgentId, Author,
    BusCommand, BusEvent, Draft, Method, Provider, ResponseResult, RoomId, RuntimeStatus, Worker,
    MASTER_AGENT_NEEDS_ROOM,
};
use crate::messaging::orchestration::{self as orchestrator, OrchestratorSpec};
use serde_json::{json, Value};

/// Fences completion to the obligation captured before issuing the interrupt.
#[derive(PartialEq, Eq)]
pub(in crate::messaging::coordinator) struct NativeInterruptFence {
    attempt: InterruptAttempt,
    identity: crate::messaging::model::AgentRuntimeIdentity,
    current: Option<crate::messaging::model::RequestId>,
    busy_revision: u64,
    queued: Vec<crate::messaging::model::RequestId>,
    members: Vec<crate::messaging::model::RequestId>,
    submission: Option<InterruptSubmission>,
}

struct InterruptAttempt {
    epoch: std::sync::Arc<()>,
    serial: u64,
}

impl PartialEq for InterruptAttempt {
    fn eq(&self, other: &Self) -> bool {
        self.serial == other.serial && std::sync::Arc::ptr_eq(&self.epoch, &other.epoch)
    }
}
impl Eq for InterruptAttempt {}

#[derive(PartialEq, Eq)]
struct InterruptSubmission {
    revision: u64,
    boundary: Option<u64>,
    turn: Option<String>,
    session: Option<String>,
    launch: Option<String>,
}

impl Worker {
    fn admit_interrupt_attempt(&mut self, id: AgentId) -> Result<(), String> {
        let attempts = &mut self.interrupt_attempts;
        attempts.serial = attempts
            .serial
            .checked_add(1)
            .ok_or("Interrupt admission serial exhausted")?;
        attempts
            .latest
            .retain(|id, _| self.state.agent(*id).is_some());
        attempts.latest.insert(id, attempts.serial);
        Ok(())
    }

    pub(in crate::messaging::coordinator) fn clear_unconfirmed_native_interrupt(
        &self,
        state: &mut crate::messaging::model::BusState,
        id: AgentId,
        fence: &NativeInterruptFence,
    ) -> Result<bool, String> {
        if self.native_interrupt_fence(state, id).as_ref() != Some(fence) {
            return Ok(false);
        }
        state
            .clear_native_interrupt_wait(id)
            .map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub(in crate::messaging::coordinator) fn native_interrupt_fence(
        &self,
        state: &crate::messaging::model::BusState,
        id: AgentId,
    ) -> Option<NativeInterruptFence> {
        let agent = state
            .agent(id)
            .filter(|agent| agent.provider == Provider::Codex)?;
        agent
            .runtime_identity
            .terminal_id
            .as_deref()
            .filter(|id| !id.is_empty())?;
        agent
            .runtime_identity
            .session_id
            .as_deref()
            .filter(|id| !id.is_empty())?;
        let request = agent.current_request.and_then(|id| state.request(id));
        Some(NativeInterruptFence {
            attempt: InterruptAttempt {
                epoch: self.interrupt_attempts.epoch.clone(),
                serial: *self.interrupt_attempts.latest.get(&id)?,
            },
            identity: agent.runtime_identity.clone(),
            current: agent.current_request,
            busy_revision: agent.busy_revision,
            queued: state.queued_requests(id).to_vec(),
            members: agent
                .current_request
                .map_or_else(Vec::new, |id| state.group_members(id)),
            submission: request.map(|request| InterruptSubmission {
                revision: request.submission_status_revision,
                boundary: request.submission_boundary,
                turn: request.provider_turn_id.clone(),
                session: request.provider_session_id.clone(),
                launch: request.expected_launch_id.clone(),
            }),
        })
    }

    pub(in crate::messaging::coordinator) fn complete_native_interrupt(
        &self,
        state: &mut crate::messaging::model::BusState,
        id: AgentId,
        fence: &NativeInterruptFence,
        terminal: &str,
        session: &str,
        turn: Option<&str>,
        now: u64,
    ) -> Result<bool, String> {
        if terminal.is_empty()
            || session.is_empty()
            || fence.identity.terminal_id.as_deref() != Some(terminal)
            || fence.identity.session_id.as_deref() != Some(session)
            || self.native_interrupt_fence(state, id).as_ref() != Some(fence)
            || fence
                .submission
                .as_ref()
                .is_some_and(|request| request.turn.as_deref() != turn)
        {
            return Ok(false);
        }
        state
            .complete_native_interrupt(id, now)
            .map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub(super) fn execute_agents(
        &mut self,
        method: &str,
        p: &Value,
        _events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<Value, String> {
        match method {
            "agent.list" => {
                let room = optional_text(p, "room")?
                    .map(|s| self.control_room(s))
                    .transpose()?;
                Ok(
                    json!({"agents":self.state.agents().filter(|a| room.is_none_or(|room| a.room_id == room)).map(|a|json!({"name":a.name,"id":a.id,"room":a.room_id})).collect::<Vec<_>>()}),
                )
            }
            "agent.details" => self.control_command(BusCommand::SetDetails(
                self.control_agent(p)?,
                p.get("on")
                    .and_then(Value::as_bool)
                    .ok_or("Details must be on or off")?,
            )),
            "agent.rename" => self.control_command(BusCommand::RenameAgent(
                self.control_agent(p)?,
                required(p, "name")?.into(),
            )),
            "agent.delete" => self.control_command(BusCommand::DeleteAgent(self.control_agent(p)?)),
            "agent.setup-confirm" => {
                self.control_command(BusCommand::CompleteHookSetup(self.control_agent(p)?))
            }
            "agent.add" => {
                let provider = match required(p, "provider")? {
                    "claude" => Provider::ClaudeCode,
                    "claude-ds" => Provider::ClaudeDs,
                    "codex" => Provider::Codex,
                    "cursor" => Provider::Cursor,
                    "grok" => Provider::Grok,
                    "kimi" => Provider::Kimi,
                    #[cfg(feature = "dev-tools")]
                    "fake"
                        if crate::agents::providers::launch::native_provider("fake").is_some() =>
                    {
                        Provider::Fake
                    }
                    _ => return Err(
                        "Provider must be claude, claude-ds, codex, cursor, grok, kimi, or fake"
                            .into(),
                    ),
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
                let agent = self.control_agent(p)?;
                self.control_clear(agent)
            }
            "agent.interrupt" => self.control_interrupt(p),
            "agent.resume" => {
                let room = self.control_room(required(p, "room")?)?;
                self.resume_room(room)
            }
            _ => Err("Unknown method".into()),
        }
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
            Some(session) => {
                Method::AgentPromptIfIdleAtGeneration(schema::AgentPromptIfIdleParams {
                    target: pane.clone(),
                    text: text.clone(),
                    expected_terminal_id: terminal.clone(),
                    expected_pane_id: pane.clone(),
                    expected_agent: launch::provider_kind(agent.provider).into(),
                    expected_session_id: session.clone(),
                    steer: false,
                    expected_generation: Some(self.serving_generation),
                    expected_turn_id: None,
                })
            }
            // Codex starts its provider session with the first turn.
            None if agent.provider == Provider::Codex => {
                Method::AgentPromptIfUnboundAtGeneration(schema::AgentPromptIfUnboundParams {
                    target: pane.clone(),
                    text: text.clone(),
                    expected_terminal_id: terminal.clone(),
                    expected_pane_id: pane.clone(),
                    expected_managed_name: format!("bus-r{}-a{}", agent.room_id.0, agent.id.0),
                    expected_generation: Some(self.serving_generation),
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
        self.requeue_first_message_instructions(id)?;
        Ok(json!({"agent_id": id, "sent": text, "stage": "cleared"}))
    }

    /// Providers that cannot take the prompt at launch get it as a first
    /// message. Clear starts a fresh context, so that text has to be queued
    /// again. Cursor and Grok never take launch arguments. An adopted Codex
    /// session replays its original developer instructions and took the same
    /// first-message path. Claude, Kimi, and a Codex session Bus started keep
    /// the form in their prompt arguments.
    fn requeue_first_message_instructions(&mut self, id: AgentId) -> Result<(), String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?.clone();
        if agent.provider == Provider::Kimi {
            return Ok(());
        }
        let adopted = agent
            .runtime_identity
            .launch_id
            .as_deref()
            .is_some_and(|launch| {
                launch::reserved_session(&self.data_dir.join("callbacks").join(launch)).is_some()
            });
        if orchestrator::prompt_args(agent.provider, std::path::Path::new("."), adopted)?.is_some()
        {
            return Ok(());
        }
        let docs = orchestrator::write_docs(&self.data_dir)?;
        let saved = agent
            .runtime_identity
            .launch_id
            .as_deref()
            .and_then(|launch| {
                std::fs::read_to_string(
                    self.data_dir
                        .join("callbacks")
                        .join(launch)
                        .join(orchestrator::PROMPT_FILE),
                )
                .ok()
            });
        let is_orchestrator = agent.orchestrates.is_some();
        let instructions = if let Some(saved) = saved {
            saved
        } else if is_orchestrator {
            let onboarding = orchestrator::agent_onboarding(&agent.name, id, agent.room_id, &docs);
            let values = orchestrator::PromptValues {
                room: agent.orchestrates.and_then(|room| {
                    self.state
                        .room(room)
                        .map(|room| (room.name.clone(), room.id))
                }),
                agent: agent.name.clone(),
                docs,
            };
            let body = format!(
                "{onboarding}\n\n{}",
                orchestrator::fill(orchestrator::DEFAULT_PROMPT, &values)
            );
            if agent.provider == Provider::Grok {
                crate::agents::providers::grok::system_prompt::orchestrator_prompt(&body)?
            } else {
                body
            }
        } else {
            orchestrator::agent_onboarding(&agent.name, id, agent.room_id, &docs)
        };
        self.state
            .submit_bus_message_with(
                agent.room_id,
                Draft {
                    text: super::super::agents::first_message_text(
                        agent.provider,
                        is_orchestrator,
                        &instructions,
                    ),
                    files: Vec::new(),
                    recipient_ids: [id].into_iter().collect(),
                },
                Author::Developer,
                crate::messaging::storage::io::now_ms(),
                true,
            )
            .map_err(|error| error.to_string())?;
        Ok(())
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
            (Some(_), _) => {
                let id = self.control_agent(p)?;
                (
                    self.state.agent(id).ok_or("Unknown agent")?.room_id,
                    Some(id),
                )
            }
            (None, Some(room)) => (self.control_room(room)?, None),
            _ => return Err("Name one agent, or a room with --room".into()),
        };
        let conversation = optional_text(p, "room")?
            .map(|selector| self.control_room(selector))
            .transpose()?
            .unwrap_or(room);
        let author = optional_text(p, "as")?
            .map(|selector| self.control_author(conversation, selector))
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
                .filter(|a| a.provider_retry.is_some() || needs_stop(&self.state, a))
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
            rows.push(self.interrupt_agent(&mut state, id, room, &interrupter, now)?);
        }
        self.save(state)?;
        Ok(
            json!({"room_id": room, "conversation_room_id": conversation, "author_id": author, "agents": rows}),
        )
    }

    /// One agent's part of an interrupt, as its result row: the stop key for
    /// a working or blocked agent, the held turn closed for an idle one.
    fn interrupt_agent(
        &mut self,
        state: &mut crate::messaging::model::BusState,
        id: AgentId,
        room: RoomId,
        interrupter: &crate::messaging::model::Author,
        now: u64,
    ) -> Result<Value, String> {
        let agent = state.agent(id).ok_or("Unknown agent")?;
        let mut row = json!({"agent_id": id, "name": agent.name});
        let status = agent.activity_status();
        match status {
            RuntimeStatus::Unavailable if agent.provider_retry.is_some() => {
                state
                    .close_interrupted(id, now)
                    .map_err(|e| e.to_string())?;
                state
                    .post_interrupt_notice(room, interrupter.clone(), id, now)
                    .map_err(|e| e.to_string())?;
                row["outcome"] = json!("already_idle");
            }
            RuntimeStatus::Idle if !needs_stop(state, agent) => {
                // Nothing to stop. A turn Bus still holds (settling, or
                // waiting on background shells) closes. Messages that never
                // joined it stay queued.
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
            RuntimeStatus::Idle | RuntimeStatus::Working | RuntimeStatus::Blocked => {
                return self.interrupt_running_agent(state, id, room, interrupter, now);
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
        Ok(row)
    }

    fn interrupt_running_agent(
        &mut self,
        state: &mut crate::messaging::model::BusState,
        id: AgentId,
        room: RoomId,
        interrupter: &crate::messaging::model::Author,
        now: u64,
    ) -> Result<Value, String> {
        let agent = state.agent(id).ok_or("Unknown agent")?;
        let mut row = json!({"agent_id":id,"name":agent.name});
        let key = interrupt_key(agent.provider);
        let Some(pane) = agent.runtime_identity.pane_id.clone() else {
            row["outcome"] = json!("failed");
            row["reason"] = json!("The agent has no terminal yet");
            return Ok(row);
        };
        self.admit_interrupt_attempt(id)?;
        let fence = self.native_interrupt_fence(state, id);
        // One press only: a second Esc opens Claude's rewind or
        // Codex's edit-previous-message, and a second Ctrl+C quits
        // Cursor or escalates Grok's cancel toward quit.
        let sent = self
            .transport
            .request(Method::AgentSendKeys(schema::AgentSendKeysParams {
                target: pane,
                keys: vec![key.into()],
                native_interrupt: (agent.provider == Provider::Codex)
                    .then(|| {
                        Some(schema::NativeInterruptIdentity {
                            expected_terminal_id: agent.runtime_identity.terminal_id.clone()?,
                            expected_session_id: agent.runtime_identity.session_id.clone()?,
                        })
                    })
                    .flatten(),
            }));
        match sent {
            Ok(ResponseResult::AgentInterrupted {
                terminal_id,
                session_id,
                turn_id,
            }) => {
                let completed = if let Some(fence) = fence {
                    self.complete_native_interrupt(
                        state,
                        id,
                        &fence,
                        &terminal_id,
                        &session_id,
                        turn_id.as_deref(),
                        now,
                    )?
                } else {
                    false
                };
                if completed {
                    state
                        .post_interrupt_notice(room, interrupter.clone(), id, now)
                        .map_err(|e| e.to_string())?;
                    row["outcome"] = json!("stopped");
                } else {
                    row["outcome"] = json!("failed");
                    row["reason"] = json!(
                        "Native interrupt completion did not match the interrupted obligation"
                    );
                }
            }
            Err(error)
                if error.definitely_rejected
                    || fence.is_some()
                    || error.code.as_deref() == Some("agent_interrupt_unconfirmed") =>
            {
                if error.code.as_deref() == Some("agent_interrupt_unconfirmed") {
                    if let Some(fence) = fence.as_ref() {
                        self.clear_unconfirmed_native_interrupt(state, id, fence)?;
                    }
                }
                tracing::warn!(
                    event = "bus.agent.interrupt",
                    agent_id = id.0,
                    key,
                    error = %error.message,
                    "Interrupt refused"
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
                    "Interrupt admission completed"
                );
                state.begin_interrupt(id).map_err(|e| e.to_string())?;
                state
                    .post_interrupt_notice(room, interrupter.clone(), id, now)
                    .map_err(|e| e.to_string())?;
                row["outcome"] = json!("interrupting");
                row["key"] = json!(key);
            }
        }
        Ok(row)
    }
}

/// Screen status is a presentation observation, not proof that a bound turn
/// stopped. An empty composer can be visible while foreground tools still run.
/// A native final releases the current request; a background-pending Stop has
/// already ended foreground work and needs only the ordinary request cleanup.
fn needs_stop(
    state: &crate::messaging::model::BusState,
    agent: &crate::messaging::model::RoomAgent,
) -> bool {
    match agent.activity_status() {
        RuntimeStatus::Working | RuntimeStatus::Blocked => true,
        RuntimeStatus::Idle
            if agent
                .provider_retry
                .as_ref()
                .is_some_and(|h| agent.current_request.is_none_or(|id| id == h.lead)) =>
        {
            false
        }
        RuntimeStatus::Idle => agent
            .current_request
            .and_then(|id| state.request(id))
            .is_some_and(|request| {
                request.trusted_start_bound
                    && request.phase != crate::messaging::model::RequestPhase::Abandoned
                    && request.pending_final.is_none()
                    && !request.awaiting_background
            }),
        _ => false,
    }
}

/// The key that stops a running turn, from each provider's own working hint:
/// Claude Code and Codex show `esc to interrupt`, Cursor `ctrl+c to stop`.
/// Grok 1.0.70 ignores Esc. Its foreground footer is `Ctrl+c:cancel`, and one
/// Ctrl+C clears `[stop]`. A second Ctrl+C paints `Ctrl+c:press again to quit`.
fn interrupt_key(provider: Provider) -> &'static str {
    match provider {
        Provider::ClaudeCode | Provider::ClaudeDs | Provider::Codex | Provider::Kimi => "esc",
        Provider::Cursor | Provider::Grok => "ctrl+c",
        #[cfg(feature = "dev-tools")]
        Provider::Fake => "esc",
    }
}

/// The provider command that starts a fresh context in the same terminal.
/// Codex's `/new` asks where the new conversation runs; `/clear` does not.
fn clear_command(provider: Provider) -> &'static str {
    match provider {
        Provider::ClaudeCode | Provider::ClaudeDs | Provider::Codex => "/clear",
        Provider::Cursor => "/new-chat",
        Provider::Grok => "/new",
        Provider::Kimi => "/new",
        #[cfg(feature = "dev-tools")]
        Provider::Fake => "/clear",
    }
}
