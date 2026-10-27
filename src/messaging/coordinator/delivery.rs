//! Coordinator delivery ownership.
use super::io;
use super::{
    callbacks, diagnostics, launch, schema, Method, Provider, RequestId, ResponseResult, RoomAgent,
    RuntimeStatus, SubmissionOutcome, Worker, OWN_TURN_GRACE,
};

/// How long a definitely rejected native prompt waits before Bus pastes it
/// again. Each attempt pastes into and clears the agent's input box, and the
/// queued stall closes ordinary requests after `QUEUED_STALL_MS`. Reply wakes
/// remain queued, but pause retries until fresh status, a new session or a
/// change to the queue.
pub(super) const PROMPT_NOT_SHOWN_RETRY: std::time::Duration = std::time::Duration::from_secs(20);

pub(super) struct ReplyDeliveryRetry {
    identity: crate::messaging::model::AgentRuntimeIdentity,
    status: RuntimeStatus,
    status_since_ms: u64,
    busy_revision: u64,
    queue_tail: Option<RequestId>,
    paused: bool,
}

impl Worker {
    pub(super) fn reply_waiter_now(&self) -> std::time::Instant {
        #[cfg(test)]
        if let Some(now) = self.test_reply_waiter_now {
            return now;
        }
        std::time::Instant::now()
    }

    /// Retaining a wake must not keep pasting into a broken composer forever.
    /// Status, identity or queue changes permit a fresh attempt; repeated
    /// idle polls alone do not.
    pub(super) fn pause_stalled_reply_deliveries(&mut self, now_ms: u64) {
        self.reply_delivery_retries.retain(|id, retry| {
            let Some(request) = self.state.request(*id) else {
                return false;
            };
            let Some(agent) = self.state.agent(request.agent_id) else {
                return false;
            };
            if request.phase != super::RequestPhase::Queued
                || !self.state.is_reply_notification(*id)
                || agent.runtime_identity != retry.identity
                || agent.shown_status() != retry.status
                || agent.status_since_ms != retry.status_since_ms
                || agent.busy_revision != retry.busy_revision
                || self.state.queued_requests(agent.id).last().copied() != retry.queue_tail
            {
                return false;
            }
            retry.paused |= self.state.stall_reason(request, now_ms).is_some();
            true
        });
    }

    #[cfg(test)]
    pub(super) fn submit_ready(&mut self) -> Result<(), String> {
        self.submit_ready_while(|| true)
    }

    pub(super) fn submit_ready_while(
        &mut self,
        mut can_deliver: impl FnMut() -> bool,
    ) -> Result<(), String> {
        let now = self.reply_waiter_now();
        self.reply_waiters
            .retain(|_, at| now.saturating_duration_since(*at) < std::time::Duration::from_secs(3));
        let waited = self.reply_waiters.keys().copied().collect();
        let mut state = self.state.clone();
        let now_ms = super::io::now_ms();
        let mut changed = state
            .route_worker_replies(now_ms, &waited)
            .map_err(|e| e.to_string())?;
        changed |= self.prepare_orchestrator_inboxes(&mut state, now_ms);
        if changed {
            self.save(state)?;
        }
        self.expire_queued_now()?;
        self.delivery_waits
            .retain(|id, _| self.state.agent(*id).is_some());
        self.withdrawn_at.retain(|id, at| {
            self.state.agent(*id).is_some() && at.elapsed() < PROMPT_NOT_SHOWN_RETRY
        });
        let agents: Vec<_> = self.state.agents().cloned().collect();
        for agent in agents {
            // A command may have arrived while polling or sending to a prior
            // agent. Re-enter command dispatch before starting another send.
            if !can_deliver() {
                break;
            }
            // Messages waited because the agent could not take them yet.
            let waited = self.delivery_waits.contains_key(&agent.id);
            if let Some(request) = self.state.queued_requests(agent.id).first().copied() {
                if let Some(reason) = diagnostics::wait_reason(&agent) {
                    if self.delivery_waits.insert(agent.id, (request, reason))
                        != Some((request, reason))
                    {
                        diagnostics::request(&self.state, request, "bus.delivery.wait", reason);
                    }
                } else {
                    self.delivery_waits.remove(&agent.id);
                }
            } else {
                self.delivery_waits.remove(&agent.id);
            }
            if self.withdrawn_at.contains_key(&agent.id) {
                continue;
            }
            if self
                .state
                .queued_requests(agent.id)
                .first()
                .is_some_and(|id| {
                    self.reply_delivery_retries
                        .get(id)
                        .is_some_and(|retry| retry.paused)
                })
            {
                continue;
            }
            if let Some((request, lead)) = self.state.next_steering(agent.id) {
                if agent.hook_setup_confirmed && !agent.session_binding_invalidated {
                    self.steer(&agent, request, lead)?;
                }
                continue;
            }
            let own_turn = self
                .own_turns
                .get(&agent.id)
                .is_some_and(|at| at.elapsed() < OWN_TURN_GRACE);
            // Claude accepts follow-ups during a turn it started outside Bus,
            // including while the native idle status still lags its start hook.
            // The new request binds only when its own payload is reported.
            let steer_unbound = agent.provider == Provider::ClaudeCode
                && agent.current_request.is_none()
                && !agent.dialog
                && matches!(agent.status, RuntimeStatus::Idle | RuntimeStatus::Working)
                && (agent.status == RuntimeStatus::Working || own_turn)
                && self.state.next_queued_request(agent.id).is_some_and(|id| {
                    self.state.request(id).is_some_and(|request| {
                        !request.queue_only && request.room_id == agent.room_id
                    })
                });
            // The native status lags a turn the agent just began on its own (a
            // task notification, say); text typed then joins that turn and is
            // never seen starting. Its Stop, or the status catching up, ends this.
            if own_turn && !steer_unbound {
                continue;
            }
            // A visible dialog takes typed text as its answer: Codex reports
            // Idle under its startup update chooser, whose Enter updates.
            if (agent.status != RuntimeStatus::Idle && !steer_unbound)
                || agent.dialog
                || !agent.hook_setup_confirmed
                || agent.session_binding_invalidated
                || agent.deletion_pending
            {
                continue;
            }
            self.deliver_request(&agent, waited, steer_unbound)?;
        }
        Ok(())
    }

    fn prepare_orchestrator_inboxes(&self, state: &mut super::BusState, now_ms: u64) -> bool {
        let mut changed = false;
        let inboxes: Vec<_> = state
            .agents()
            .filter(|a| a.orchestrates.is_some())
            .map(|a| a.id)
            .collect();
        for agent in inboxes {
            changed |= state.prioritize_orchestrator_dialogs(agent);
            // A submit hook can precede the native Working observation. Do not
            // split an autonomous turn during that existing grace period.
            if self
                .own_turns
                .get(&agent)
                .is_none_or(|at| at.elapsed() >= OWN_TURN_GRACE)
            {
                changed |= state.release_orchestrator_inbox(agent, now_ms).is_some();
            }
        }
        changed
    }

    fn deliver_request(
        &mut self,
        agent: &RoomAgent,
        waited: bool,
        steer: bool,
    ) -> Result<(), String> {
        let identity = &agent.runtime_identity;
        let (Some(launch), Some(terminal), Some(pane)) = (
            &identity.launch_id,
            &identity.terminal_id,
            &identity.pane_id,
        ) else {
            return Ok(());
        };
        if identity.session_id.is_none() && agent.provider != Provider::Codex {
            return Ok(());
        }
        let Some(request) = self.state.next_queued_request(agent.id) else {
            return Ok(());
        };
        let mut state = self.state.clone();
        // Messages that piled up while the agent was unavailable go as one prompt.
        let text = match waited.then(|| state.coalesce_queue(request)).flatten() {
            Some(text) => text,
            None => state
                .render_request_payload(request)
                .ok_or("Missing queued request")?,
        };
        let boundary = callbacks::boundary(&self.data_dir.join("callbacks").join(launch))
            .map_err(|e| e.to_string())?;
        state
            .begin_submission(request, launch, boundary)
            .map_err(|e| e.to_string())?;
        state.begin_dialog_notice_write(request, io::now_ms());
        self.save(state)?;
        diagnostics::request(&self.state, request, "bus.delivery.start", "native_submit");
        let started = std::time::Instant::now();
        let _span = tracing::info_span!(
            "bus.delivery",
            request_id = request.0,
            agent_id = agent.id.0,
            room_id = agent.room_id.0
        )
        .entered();
        tracing::debug!(
            event = "bus.delivery.payload",
            payload_bytes = text.len(),
            "Bus payload metadata"
        );
        let method = submission_method(agent, text, terminal, pane, steer);
        let outcome = match self.transport.request(method) {
            Ok(ResponseResult::AgentPrompted { .. }) => SubmissionOutcome::Confirmed {
                provider_session_id: identity.session_id.clone(),
                provider_turn_id: None,
            },
            Ok(other) => SubmissionOutcome::Uncertain {
                message: format!("Unexpected submit response; no automatic retry: {other:?}"),
            },
            Err(error)
                if error.code.as_deref() == Some(crate::protocol::api::AGENT_COMPOSER_BUSY) =>
            {
                SubmissionOutcome::ComposerBusy
            }
            Err(error) if error.definitely_rejected => SubmissionOutcome::DefinitelyRejected {
                message: error.message,
            },
            Err(error) => SubmissionOutcome::Uncertain {
                message: format!(
                    "Submit outcome unknown; no automatic retry: {}",
                    error.message
                ),
            },
        };
        let outcome_name = match &outcome {
            SubmissionOutcome::ComposerBusy => "composer_wait",
            SubmissionOutcome::Confirmed { .. } => "confirmed",
            SubmissionOutcome::DefinitelyRejected { .. } => "rejected",
            SubmissionOutcome::Uncertain { .. } => "uncertain",
        };
        tracing::info!(
            event = "bus.delivery.result",
            request_id = request.0,
            outcome = outcome_name,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Native submit result; uncertain outcomes are never retried"
        );
        let mut state = self.state.clone();
        state
            .record_submission(request, outcome.clone())
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        self.note_withdrawn(agent, request, &outcome);
        Ok(())
    }

    fn note_withdrawn(
        &mut self,
        agent: &RoomAgent,
        request: RequestId,
        outcome: &SubmissionOutcome,
    ) {
        if let SubmissionOutcome::DefinitelyRejected { message } = outcome {
            if self.state.is_reply_notification(request) {
                self.reply_delivery_retries.insert(
                    request,
                    ReplyDeliveryRetry {
                        identity: agent.runtime_identity.clone(),
                        status: agent.shown_status(),
                        status_since_ms: agent.status_since_ms,
                        busy_revision: agent.busy_revision,
                        queue_tail: self.state.queued_requests(agent.id).last().copied(),
                        paused: false,
                    },
                );
            }
            if message.contains(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE) {
                self.withdrawn_at
                    .insert(agent.id, std::time::Instant::now());
            }
        }
    }

    /// Types `request` into the agent's running turn, the way a person types
    /// while an agent works, so it joins `lead`'s group and shares its reply.
    pub(super) fn steer(
        &mut self,
        agent: &RoomAgent,
        request: RequestId,
        lead: RequestId,
    ) -> Result<(), String> {
        let identity = &agent.runtime_identity;
        let (Some(terminal), Some(pane), Some(session)) = (
            &identity.terminal_id,
            &identity.pane_id,
            &identity.session_id,
        ) else {
            return Ok(());
        };
        let mut state = self.state.clone();
        let text = state
            .render_request_payload(request)
            .ok_or("Missing queued request")?;
        state
            .begin_steering(request, lead)
            .map_err(|e| e.to_string())?;
        state.begin_dialog_notice_write(request, io::now_ms());
        self.save(state)?;
        diagnostics::request(&self.state, request, "bus.delivery.start", "steer");
        let started = std::time::Instant::now();
        let outcome = match self.transport.request(Method::AgentPromptIfIdle(
            schema::AgentPromptIfIdleParams {
                target: pane.clone(),
                text,
                expected_terminal_id: terminal.clone(),
                expected_pane_id: pane.clone(),
                expected_agent: launch::provider_kind(agent.provider).into(),
                expected_session_id: session.clone(),
                steer: true,
            },
        )) {
            Ok(ResponseResult::AgentPrompted { .. }) => SubmissionOutcome::Confirmed {
                provider_session_id: Some(session.clone()),
                provider_turn_id: None,
            },
            Ok(other) => SubmissionOutcome::Uncertain {
                message: format!("Unexpected steering response: {other:?}"),
            },
            Err(error)
                if error.code.as_deref() == Some(crate::protocol::api::AGENT_COMPOSER_BUSY) =>
            {
                SubmissionOutcome::ComposerBusy
            }
            Err(error) if error.definitely_rejected => SubmissionOutcome::DefinitelyRejected {
                message: error.message,
            },
            Err(error) => SubmissionOutcome::Uncertain {
                message: error.message,
            },
        };
        tracing::info!(event = "bus.delivery.result", request_id = request.0, lead_id = lead.0,
            mode = "steer", outcome = ?outcome,
            elapsed_ms = started.elapsed().as_millis() as u64, "Typed into the running turn");
        let mut state = self.state.clone();
        state
            .record_steering(request, outcome.clone())
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        self.note_withdrawn(agent, request, &outcome);
        Ok(())
    }
}

fn submission_method(
    agent: &RoomAgent,
    text: String,
    terminal: &str,
    pane: &str,
    steer: bool,
) -> Method {
    if let Some(session) = &agent.runtime_identity.session_id {
        Method::AgentPromptIfIdle(schema::AgentPromptIfIdleParams {
            target: pane.to_owned(),
            text,
            expected_terminal_id: terminal.to_owned(),
            expected_pane_id: pane.to_owned(),
            expected_agent: launch::provider_kind(agent.provider).into(),
            expected_session_id: session.clone(),
            steer,
        })
    } else {
        Method::AgentPromptIfUnbound(schema::AgentPromptIfUnboundParams {
            target: pane.to_owned(),
            text,
            expected_terminal_id: terminal.to_owned(),
            expected_pane_id: pane.to_owned(),
            expected_managed_name: format!("bus-r{}-a{}", agent.room_id.0, agent.id.0),
        })
    }
}
