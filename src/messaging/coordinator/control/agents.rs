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
                    "agy" => Provider::Agy,
                    #[cfg(feature = "dev-tools")]
                    "fake"
                        if crate::agents::providers::launch::native_provider("fake").is_some() =>
                    {
                        Provider::Fake
                    }
                    _ => return Err(
                        "Provider must be claude, claude-ds, codex, cursor, grok, kimi, agy, or fake"
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
            // Codex and AGY start their provider session with the first turn.
            None if matches!(agent.provider, Provider::Codex | Provider::Agy) => {
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
                .filter(|a| a.provider_retry.is_some() || self.state.needs_interrupt_stop(a))
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
            rows.push(self.interrupt_agent(
                &mut state,
                id,
                room,
                &interrupter,
                now,
                true,
                None,
                None,
            )?);
        }
        self.save(state)?;
        Ok(
            json!({"room_id": room, "conversation_room_id": conversation, "author_id": author, "agents": rows}),
        )
    }

    /// One agent's part of an interrupt, as its result row: the stop key for
    /// a working or blocked agent, the held turn closed for an idle one.
    pub(super) fn interrupt_agent(
        &mut self,
        state: &mut crate::messaging::model::BusState,
        id: AgentId,
        room: RoomId,
        interrupter: &crate::messaging::model::Author,
        now: u64,
        announce: bool,
        may_have_written: Option<&mut bool>,
        staged: Option<&crate::messaging::model::SendInterrupt>,
    ) -> Result<Value, String> {
        let agent = state.agent(id).ok_or("Unknown agent")?;
        let mut row = json!({"agent_id": id, "name": agent.name});
        let status = agent.activity_status();
        match status {
            RuntimeStatus::Unavailable if state.interrupt_retirement_owner(id).is_some() => {
                row["outcome"] = json!("failed");
                row["reason"] = json!(
                    crate::messaging::model::ModelError::InterruptRetirementPending.to_string()
                );
            }
            RuntimeStatus::Unavailable if agent.provider_retry.is_some() => {
                state
                    .close_interrupted(id, now)
                    .map_err(|e| e.to_string())?;
                self.interrupt_notice(state, id, room, interrupter, now, announce)?;
                row["outcome"] = json!("already_idle");
            }
            RuntimeStatus::Idle if !state.needs_interrupt_stop(agent) => {
                // Nothing to stop. A turn Bus still holds (settling, or
                // waiting on background shells) closes. Messages that never
                // joined it stay queued.
                let closed = state
                    .close_interrupted(id, now)
                    .map_err(|e| e.to_string())?;
                // Their waiters point to the history for who stopped them.
                if !closed.is_empty() {
                    self.interrupt_notice(state, id, room, interrupter, now, announce)?;
                }
                row["outcome"] = json!("already_idle");
            }
            RuntimeStatus::Idle | RuntimeStatus::Working | RuntimeStatus::Blocked => {
                return self.interrupt_running_agent(
                    state,
                    id,
                    room,
                    interrupter,
                    now,
                    announce,
                    may_have_written,
                    staged,
                );
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

    /// `agent interrupt` posts "Interrupted AGENT." so waiters learn who
    /// stopped the turn. A send-triggered interrupt stays silent: the message
    /// itself explains the turn ending and already publishes in that room.
    fn interrupt_notice(
        &self,
        state: &mut crate::messaging::model::BusState,
        id: AgentId,
        room: RoomId,
        interrupter: &crate::messaging::model::Author,
        now: u64,
        announce: bool,
    ) -> Result<(), String> {
        if announce {
            state
                .post_interrupt_notice(room, interrupter.clone(), id, now)
                .map_err(|e| e.to_string())?;
        } else {
            state.cancel_pending_launch(id, crate::messaging::model::INTERRUPTED, now);
        }
        Ok(())
    }

    fn persist_claude_retirement_proof(
        &mut self,
        state: &mut crate::messaging::model::BusState,
        id: AgentId,
    ) -> Result<(), String> {
        // The caller may journal only the follower send. A failed save leaves
        // the old hold and a retryable native receipt.
        let callbacks = self.retirement_callback_evidence(&self.state, id, false)?;
        let mut durable = self.state.clone();
        durable.reconcile_retirement_answers(id, &callbacks);
        durable
            .set_interrupt_retirement(id, false)
            .map_err(|e| e.to_string())?;
        durable.begin_interrupt(id).map_err(|e| e.to_string())?;
        self.save(durable)?;
        state.reconcile_retirement_answers(id, &callbacks);
        state
            .set_interrupt_retirement(id, false)
            .map_err(|e| e.to_string())
    }

    fn prepare_claude_retirement(
        &mut self,
        state: &mut crate::messaging::model::BusState,
        id: AgentId,
        request: super::RequestId,
        runtime: &crate::messaging::model::AgentRuntimeIdentity,
    ) -> Result<Option<&'static str>, String> {
        // Do not write Esc until the original prompt is durably tracked.
        let mut durable = self.state.clone();
        for target in [&mut *state, &mut durable] {
            target
                .set_interrupt_retirement(id, true)
                .map_err(|e| e.to_string())?;
            target
                .clear_native_interrupt_wait(id)
                .map_err(|e| e.to_string())?;
        }
        self.save(durable)?;
        if runtime.terminal_id.as_deref().is_none_or(str::is_empty)
            || runtime.session_id.as_deref().is_none_or(str::is_empty)
        {
            return Ok(Some(
                "Claude's original native session is not yet confirmed; input remains tracked",
            ));
        }
        Ok(match state.interrupt_retirement_origin(request) {
            Some(original) if original == runtime => None,
            Some(_) => Some("Original Claude terminal changed; resolve retirement of the original terminal first"),
            None => Some("Cannot establish one original delivery terminal for this request group"),
        })
    }

    fn interrupt_running_agent(
        &mut self,
        state: &mut crate::messaging::model::BusState,
        id: AgentId,
        room: RoomId,
        interrupter: &crate::messaging::model::Author,
        now: u64,
        announce: bool,
        may_have_written: Option<&mut bool>,
        staged: Option<&crate::messaging::model::SendInterrupt>,
    ) -> Result<Value, String> {
        let agent = state.agent(id).ok_or("Unknown agent")?;
        let mut row = json!({"agent_id":id,"name":agent.name});
        let key = interrupt_key(agent.provider);
        let claude = agent.provider.uses_claude_harness();
        let current = agent.current_request;
        let runtime = agent.runtime_identity.clone();
        let native_interrupt = (agent.provider == Provider::Codex || claude)
            .then(|| Some((runtime.terminal_id.clone()?, runtime.session_id.clone()?)))
            .flatten();
        if let Some(request) = current.filter(|_| claude) {
            if let Some(reason) = self.prepare_claude_retirement(state, id, request, &runtime)? {
                row["outcome"] = json!("failed");
                row["reason"] = json!(reason);
                return Ok(row);
            }
        }
        let Some(pane) = runtime.pane_id else {
            row["outcome"] = json!("failed");
            row["reason"] = json!("The agent has no terminal yet");
            return Ok(row);
        };
        let expected_payloads = if claude {
            current
                .into_iter()
                .chain(current.map_or_else(Vec::new, |id| state.group_members(id)))
                .filter_map(|id| state.render_request_payload(id))
                .collect()
        } else {
            Vec::new()
        };
        self.admit_interrupt_attempt(id)?;
        let fence = self.native_interrupt_fence(state, id);
        // One press only: a second Esc opens Claude's rewind or
        // Codex's edit-previous-message, and a second Ctrl+C quits
        // Cursor or escalates Grok's cancel toward quit.
        let sent = self.send_stop_keys(
            pane,
            key,
            runtime.terminal_id,
            staged,
            native_interrupt.map(|(expected_terminal_id, expected_session_id)| {
                schema::NativeInterruptIdentity {
                    expected_terminal_id,
                    expected_session_id,
                    expected_payloads,
                    retirement_request_id: current.filter(|_| claude).map(|id| id.0),
                }
            }),
        );
        if let Some(written) = may_have_written {
            *written = sent.as_ref().err().is_none_or(|error| {
                !error.definitely_rejected
                    || error.code.as_deref() == Some("agent_interrupt_unconfirmed")
            });
        }
        if claude {
            let retired = matches!(&sent, Ok(ResponseResult::Ok {}));
            if retired && current.is_some() {
                self.persist_claude_retirement_proof(state, id)?;
            }
            if !retired {
                state
                    .clear_native_interrupt_wait(id)
                    .map_err(|e| e.to_string())?;
                row["outcome"] = json!("failed");
                row["reason"] = json!(format!(
                    "Claude interrupt retirement is unconfirmed; request remains tracked: {}",
                    match &sent {
                        Err(error) => error.message.as_str(),
                        _ => "Unexpected native interrupt response",
                    }
                ));
                return Ok(row);
            }
        }
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
                    self.interrupt_notice(state, id, room, interrupter, now, announce)?;
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
                self.interrupt_notice(state, id, room, interrupter, now, announce)?;
                row["outcome"] = json!("interrupting");
                row["key"] = json!(key);
            }
        }
        Ok(row)
    }
}

/// The key that stops a running turn, from each provider's own working hint:
/// Claude Code and Codex show `esc to interrupt`, Cursor `ctrl+c to stop`.
/// Grok 1.0.70 ignores Esc. Its foreground footer is `Ctrl+c:cancel`, and one
/// Ctrl+C clears `[stop]`. A second Ctrl+C paints `Ctrl+c:press again to quit`.
fn interrupt_key(provider: Provider) -> &'static str {
    match provider {
        Provider::ClaudeCode
        | Provider::ClaudeDs
        | Provider::Codex
        | Provider::Kimi
        | Provider::Agy => "esc",
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
        Provider::Grok | Provider::Kimi | Provider::Agy => "/new",
        #[cfg(feature = "dev-tools")]
        Provider::Fake => "/clear",
    }
}
