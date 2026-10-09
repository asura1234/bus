//! Coordinator delivery ownership.
use super::{
    callbacks, diagnostics, launch, schema, Method, Provider, RequestId, ResponseResult, RoomAgent,
    RuntimeStatus, SubmissionOutcome, Worker, OWN_TURN_GRACE,
};

impl Worker {
    #[cfg(test)]
    pub(super) fn submit_ready(&mut self) -> Result<(), String> {
        self.submit_ready_while(|| true)
    }

    pub(super) fn submit_ready_while(
        &mut self,
        mut can_deliver: impl FnMut() -> bool,
    ) -> Result<(), String> {
        self.expire_queued_now()?;
        self.delivery_waits
            .retain(|id, _| self.state.agent(*id).is_some());
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
                    self.state
                        .request(id)
                        .is_some_and(|request| !request.queue_only)
                });
            // The native status lags a turn the agent just began on its own (a
            // task notification, say); text typed then joins that turn and is
            // never seen starting. Its Stop, or the status catching up, ends this.
            if own_turn && !steer_unbound {
                continue;
            }
            if (agent.status != RuntimeStatus::Idle && !steer_unbound)
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
                .request(request)
                .ok_or("Missing queued request")?
                .prompt
                .rendered_payload(),
        };
        let boundary = callbacks::boundary(&self.data_dir.join("callbacks").join(launch))
            .map_err(|e| e.to_string())?;
        state
            .begin_submission(request, launch, boundary)
            .map_err(|e| e.to_string())?;
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
            .record_submission(request, outcome)
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Ok(())
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
            .request(request)
            .ok_or("Missing queued request")?
            .prompt
            .rendered_payload();
        state
            .begin_steering(request, lead)
            .map_err(|e| e.to_string())?;
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
            .record_steering(request, outcome)
            .map_err(|e| e.to_string())?;
        self.save(state)
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
