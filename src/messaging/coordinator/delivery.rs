//! Coordinator delivery ownership.
use super::io;
use super::{
    callbacks, diagnostics, Provider, RequestId, ResponseResult, RoomAgent, RuntimeStatus,
    SubmissionOutcome, Worker, OWN_TURN_GRACE,
};
use crate::messaging::model::DispatchMark;
use crate::messaging::native::TransportError;

pub(super) mod submission;
use submission::submission_method;

/// How long a definitely rejected native prompt waits before Bus pastes it
/// again. Each attempt pastes into and clears the agent's input box, and the
/// queued recovery closes ordinary requests after `QUEUED_STALL_MS`. Reply wakes
/// remain queued, but pause retries until fresh status, a new session or a
/// change to the queue.
pub(super) const PROMPT_NOT_SHOWN_RETRY: std::time::Duration = std::time::Duration::from_secs(20);

enum DeliveryRoute {
    Steer(RequestId, RequestId),
    Submit { steer: bool, allow_working: bool },
}

pub(super) struct ReplyDeliveryRetry {
    identity: crate::messaging::model::AgentRuntimeIdentity,
    status: RuntimeStatus,
    status_since_ms: u64,
    busy_revision: u64,
    queue_tail: Option<RequestId>,
    paused: bool,
}

impl Worker {
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
            retry.paused |= self.state.recovery_reason(request, now_ms).is_some();
            true
        });
    }

    #[cfg(test)]
    pub(super) fn submit_ready(&mut self) -> Result<(), String> {
        self.submit_ready_while(|| true)
    }

    pub(super) fn submit_ready_while(
        &mut self,
        can_deliver: impl FnMut() -> bool,
    ) -> Result<(), String> {
        self.submit_available_while(can_deliver, true)
    }

    pub(super) fn submit_eligible_while(
        &mut self,
        can_deliver: impl FnMut() -> bool,
    ) -> Result<(), String> {
        self.refresh_queued_recipient_callbacks()?;
        self.submit_available_while(can_deliver, false)
    }

    fn refresh_queued_recipient_callbacks(&mut self) -> Result<(), String> {
        let recipients: Vec<_> = self
            .state
            .agents()
            .filter(|agent| {
                !agent.deletion_pending
                    && !self.native_submissions.contains_key(&agent.id)
                    && !self.state.queued_requests(agent.id).is_empty()
            })
            .filter_map(|agent| {
                agent
                    .runtime_identity
                    .launch_id
                    .as_ref()
                    .map(|launch| (agent.id, self.data_dir.join("callbacks").join(launch)))
            })
            .collect();
        self.begin_batch();
        let consumed = recipients
            .into_iter()
            .try_for_each(|(agent, dir)| self.consume_callbacks(agent, &dir));
        let saved = self.finish_batch();
        consumed.and(saved)
    }

    fn submit_available_while(
        &mut self,
        mut can_deliver: impl FnMut() -> bool,
        launch_prompts: bool,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        let now_ms = super::io::now_ms();
        let mut changed = self.prepare_provider_retries(&mut state, now_ms);
        changed |= self.route_replies_with_waiters(&mut state, now_ms)?;
        changed |= self.prepare_orchestrator_inboxes(&mut state, now_ms);
        if changed {
            self.save(state)?;
        }
        self.expire_queued_now()?;
        self.restore_orchestrator_queue_order()?;
        self.delivery_waits
            .retain(|id, _| self.state.agent(*id).is_some());
        self.withdrawn_at.retain(|id, at| {
            self.state.agent(*id).is_some() && at.elapsed() < PROMPT_NOT_SHOWN_RETRY
        });
        let agents: Vec<_> = self.state.agents().cloned().collect();
        // Observe admission for every recipient before any recipient's dispatch work.
        for agent in &agents {
            self.note_delivery_eligibility(agent);
        }
        for agent in agents {
            if !self.state.queued_requests(agent.id).is_empty()
                && self.check_launch_directory(agent.id)?.is_some()
            {
                continue;
            }
            // Initial Cursor/Grok/AGY instructions stay on the readiness tick.
            // They must precede queued work and must not hold another recipient.
            if !launch_prompts
                && matches!(
                    agent.provider,
                    Provider::Cursor | Provider::Grok | Provider::Agy
                )
                && agent
                    .runtime_identity
                    .launch_id
                    .as_ref()
                    .is_some_and(|launch| {
                        !self
                            .data_dir
                            .join("callbacks")
                            .join(launch)
                            .join("launch-prompt-attempted")
                            .exists()
                    })
            {
                continue;
            }
            if !can_deliver() {
                break;
            }
            let waited = self.note_queued_delivery_wait(&agent);
            match self.delivery_route(&agent) {
                Some(DeliveryRoute::Steer(request, lead)) => self.steer(&agent, request, lead)?,
                Some(DeliveryRoute::Submit {
                    steer,
                    allow_working,
                }) if !launch_prompts || !self.deliver_launch_prompt(&agent)? => {
                    self.deliver_queued_request(&agent, waited && !steer, steer, allow_working)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn note_delivery_eligibility(&mut self, agent: &RoomAgent) {
        let request = match self.delivery_route(agent) {
            Some(DeliveryRoute::Steer(request, _)) => Some(request),
            Some(DeliveryRoute::Submit { .. }) => self.state.next_queued_request(agent.id),
            None => None,
        };
        if let Some(request) = request {
            self.state.record_eligible(request, io::now_ms());
        }
    }

    fn delivery_route(&self, agent: &RoomAgent) -> Option<DeliveryRoute> {
        let now_ms = io::now_ms();
        if agent.startup.is_some()
            || self.native_submissions.contains_key(&agent.id)
            || self.submission_rechecks.contains(&agent.id)
            || self.withdrawn_at.contains_key(&agent.id)
            || agent.provider_retry.as_ref().is_some_and(|hold| {
                !hold.queued || now_ms < hold.retry_at_ms || agent.status != RuntimeStatus::Idle
            })
            || self
                .state
                .queued_requests(agent.id)
                .first()
                .is_some_and(|id| {
                    self.reply_delivery_retries
                        .get(id)
                        .is_some_and(|retry| retry.paused)
                })
        {
            return None;
        }
        if let Some((request, lead)) = self.state.next_steering(agent.id) {
            return (agent.hook_setup_confirmed && !agent.session_binding_invalidated)
                .then_some(DeliveryRoute::Steer(request, lead));
        }
        let own_turn = self
            .own_turns
            .get(&agent.id)
            .is_some_and(|at| at.elapsed() < OWN_TURN_GRACE);
        let steer = self.steer_into_unbound_turn(agent, own_turn);
        let interrupt_ready = agent.current_request.is_none()
            && matches!(agent.status, RuntimeStatus::Idle | RuntimeStatus::Working)
            && self.state.queued_requests(agent.id).iter().any(|id| {
                self.state
                    .request(*id)
                    .is_some_and(|request| request.interrupt_delivery)
            });
        if ((agent.status != RuntimeStatus::Idle || own_turn) && !steer && !interrupt_ready)
            || agent.dialog
            || !agent.hook_setup_confirmed
            || agent.session_binding_invalidated
            || agent.deletion_pending
        {
            return None;
        }
        Some(DeliveryRoute::Submit {
            steer,
            allow_working: interrupt_ready,
        })
    }

    /// Whether the queue head can be typed into a turn Bus does not own yet.
    ///
    /// Claude accepts that follow-up while its idle status still lags a turn
    /// it started itself. Any other provider that can take input does the same
    /// once it is Working: the native `steer` flag, not an interrupt. Grok's
    /// composer refuses a follow-up for the whole turn, and AGY folds input
    /// arriving during its unbound first-message onboarding turn into a
    /// continuation (`invocationNum >= 1`) that emits no `Started` proof, so
    /// both providers stay queued on unbound turns. Queue-only work, a restart,
    /// a dialog, and an explicit interrupt stay queued too. A closed composer
    /// is the native submission window (a few hundred milliseconds); a definite
    /// refusal returns the head to the queue. This tick does not sleep.
    fn steer_into_unbound_turn(&self, agent: &RoomAgent, own_turn: bool) -> bool {
        if agent.interrupt_pending || agent.provider_retry.is_some() || agent.dialog {
            return false;
        }
        let joinable = self
            .state
            .queued_requests(agent.id)
            .first()
            .copied()
            .is_some_and(|id| {
                self.state.request(id).is_some_and(|request| {
                    self.state.can_join_turn(request) && request.room_id == agent.room_id
                })
            });
        if !joinable || agent.current_request.is_some() {
            return false;
        }
        let claude_lag = agent.provider.uses_claude_harness()
            && matches!(agent.status, RuntimeStatus::Idle | RuntimeStatus::Working)
            && (agent.status == RuntimeStatus::Working || own_turn);
        let busy = agent.status == RuntimeStatus::Working
            && !matches!(agent.provider, Provider::Grok | Provider::Agy);
        claude_lag || busy
    }

    /// Whether the queued head already waited for this agent.
    ///
    /// The flag belongs to the request about to be sent. A stale map entry for
    /// an older head must not mark this one. Dialog blocks the send without
    /// entering `wait_reason`; that function keeps its precedence, including
    /// the Codex first-session exception, so the hold is recorded here.
    fn note_queued_delivery_wait(&mut self, agent: &RoomAgent) -> bool {
        let head = self.state.queued_requests(agent.id).first().copied();
        let waited = head.is_some_and(|request| {
            self.delivery_waits
                .get(&agent.id)
                .is_some_and(|(waited_for, _)| *waited_for == request)
        });
        if let Some(request) = self.state.queued_requests(agent.id).first().copied() {
            let reason = diagnostics::wait_reason(agent).or(agent.dialog.then_some("agent_dialog"));
            if let Some(reason) = reason {
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
        waited
    }

    fn prepare_provider_retries(&self, state: &mut super::BusState, now_ms: u64) -> bool {
        let mut changed = false;
        for agent in self
            .state
            .agents()
            .filter(|a| a.provider_retry.as_ref().is_some_and(|h| h.quota))
        {
            if let Some(reset) = self.usage.retry_reset(agent, now_ms) {
                changed |= state.defer_provider_retry(agent.id, reset);
            }
        }
        changed | state.retry_provider_requests(now_ms)
    }

    fn deliver_queued_request(
        &mut self,
        agent: &RoomAgent,
        waited: bool,
        steer: bool,
        allow_working: bool,
    ) -> Result<(), String> {
        let report = self.state.next_queued_request(agent.id).filter(|id| {
            self.state
                .request(*id)
                .is_some_and(super::Request::is_error_report)
        });
        if let Err(error) = self.deliver_request(agent, waited, steer, allow_working) {
            let Some(request) = report else {
                return Err(error);
            };
            let _report = crate::utils::error_pipe::suppress(true);
            tracing::warn!(event = "bus.error_pipe.delivery_failed", request_id = request.0, %error, "Error report preparation failed");
            let mut state = self.state.clone();
            if state.recover_idle_request(request, io::now_ms()).is_ok() {
                let _ = self.save(state);
            }
        }
        Ok(())
    }

    fn restore_orchestrator_queue_order(&mut self) -> Result<(), String> {
        // Expiry can defer refused reply wakes. Restore only inbox ordering;
        // keep the existing background-owner release at its earlier boundary.
        let mut state = self.state.clone();
        let inboxes: Vec<_> = state
            .agents()
            .filter(|agent| agent.orchestrates.is_some())
            .map(|agent| agent.id)
            .collect();
        let mut changed = false;
        for agent in inboxes {
            changed |= state.prioritize_orchestrator_dialogs(agent);
        }
        if changed {
            self.save(state)?;
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

    fn deliver_launch_prompt(&mut self, agent: &RoomAgent) -> Result<bool, String> {
        if !matches!(
            agent.provider,
            Provider::Cursor | Provider::Grok | Provider::Agy
        ) || agent.current_request.is_some()
        {
            return Ok(false);
        }
        let identity = &agent.runtime_identity;
        let (Some(launch), Some(terminal), Some(pane)) = (
            &identity.launch_id,
            &identity.terminal_id,
            &identity.pane_id,
        ) else {
            return Ok(false);
        };
        let spool = self.data_dir.join("callbacks").join(launch);
        // Fresh AGY launches defer SessionStart until the first submitted prompt.
        if identity.session_id.is_none()
            && (agent.provider != Provider::Agy
                || super::launch::reserved_session(&spool).is_some())
        {
            return Ok(false);
        }
        let attempted = spool.join("launch-prompt-attempted");
        if attempted.exists() {
            return Ok(false);
        }
        let instructions =
            match std::fs::read_to_string(spool.join(super::orchestrator::PROMPT_FILE)) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error.to_string()),
            };
        if self.state.requests().any(|request| {
            request.agent_id == agent.id
                && request.bus_generated
                && request.prompt.text.ends_with(&instructions)
        }) {
            std::fs::write(&attempted, io::now_ms().to_string()).map_err(|e| e.to_string())?;
            return Ok(false);
        }
        let text = super::agents::first_message_text(
            agent.provider,
            agent.orchestrates.is_some(),
            &instructions,
        );
        std::fs::write(&attempted, io::now_ms().to_string()).map_err(|e| e.to_string())?;
        let result = self.transport.request(submission_method(
            agent,
            text,
            terminal,
            pane,
            false,
            false,
            self.serving_generation,
        ));
        match result {
            Ok(ResponseResult::AgentPrompted { .. }) => {
                self.own_turns.insert(agent.id, std::time::Instant::now());
                if agent
                    .actionable_error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("Launch prompt delivery"))
                {
                    self.set_launch_prompt_error(agent, None, None)?;
                }
                tracing::info!(
                    event = "bus.delivery.launch_prompt",
                    agent_id = agent.id.0,
                    outcome = "confirmed",
                    "Launch prompt typed without room history"
                );
            }
            Err(error) if error.definitely_rejected => {
                self.reject_launch_prompt(agent, &attempted, error)?;
            }
            Ok(other) => {
                self.own_turns.insert(agent.id, std::time::Instant::now());
                self.set_launch_prompt_error(
                    agent,
                    Some(format!(
                        "Unexpected submit response; no automatic retry: {other:?}"
                    )),
                    None,
                )?;
                tracing::warn!(event = "bus.delivery.launch_prompt", agent_id = agent.id.0,
                    response = ?other, "Launch prompt outcome unknown; not retrying");
            }
            Err(error) => {
                self.own_turns.insert(agent.id, std::time::Instant::now());
                self.set_launch_prompt_error(
                    agent,
                    Some(format!(
                        "Submit outcome unknown; no automatic retry: {}",
                        error.message
                    )),
                    None,
                )?;
                tracing::warn!(event = "bus.delivery.launch_prompt", agent_id = agent.id.0,
                    error = %error.message, "Launch prompt outcome unknown; not retrying");
            }
        }
        Ok(true)
    }

    fn reject_launch_prompt(
        &mut self,
        agent: &RoomAgent,
        attempted: &std::path::Path,
        error: crate::messaging::native::TransportError,
    ) -> Result<(), String> {
        std::fs::remove_file(attempted).map_err(|e| e.to_string())?;
        if error
            .message
            .contains(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE)
        {
            self.withdrawn_at
                .insert(agent.id, std::time::Instant::now());
        }
        let status = match error.code.as_deref() {
            Some("agent_not_found" | "agent_identity_changed") => Some(RuntimeStatus::Unavailable),
            Some("agent_blocked") => Some(RuntimeStatus::Blocked),
            Some(code)
                if code == crate::protocol::api::AGENT_COMPOSER_BUSY
                    || code == crate::protocol::api::AGENT_PROMPT_NOT_SHOWN =>
            {
                Some(RuntimeStatus::Blocked)
            }
            _ => None,
        };
        self.set_launch_prompt_error(
            agent,
            Some(format!(
                "Launch prompt delivery failed: {}. Bus will retry when the agent is ready.",
                error.message
            )),
            status,
        )?;
        tracing::info!(
            event = "bus.delivery.launch_prompt",
            agent_id = agent.id.0,
            outcome = "rejected",
            "Launch prompt was not typed; safe to retry"
        );
        Ok(())
    }

    fn set_launch_prompt_error(
        &mut self,
        agent: &RoomAgent,
        error: Option<String>,
        status: Option<RuntimeStatus>,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        if let Some(status) = status {
            state
                .observe_status(agent.id, status, io::now_ms())
                .map_err(|e| e.to_string())?;
        }
        state
            .set_agent_error(agent.id, error)
            .map_err(|e| e.to_string())?;
        self.save(state)
    }

    fn trace_provider_retry(&self, agent: &RoomAgent, request: RequestId) {
        if let Some(request) = self
            .state
            .request(request)
            .filter(|r| r.provider_retry_attempts > 0)
        {
            tracing::info!(event = "bus.provider.retry", agent_id = agent.id.0, request_id = request.id.0,
                prompt_id = request.prompt.id.0, room_id = request.room_id.0, provider = ?agent.provider,
                attempt = request.provider_retry_attempts, session_id = ?agent.runtime_identity.session_id,
                "Starting guarded provider retry");
        }
    }

    fn mark_open_dispatch(
        &mut self,
        draft: &mut crate::messaging::model::BusState,
        request: RequestId,
        mark: DispatchMark,
    ) {
        let at_ms = io::now_ms();
        self.state.record_dispatch_mark(request, mark, at_ms);
        draft.record_dispatch_mark(request, mark, at_ms);
    }

    fn mark_live_dispatch(&mut self, request: RequestId, mark: DispatchMark) {
        self.state.record_dispatch_mark(request, mark, io::now_ms());
    }

    /// Builds the prompt and stamps every dispatch step that finishes before save.
    ///
    /// Render and the callback boundary run before `record_picked`, so they are
    /// outside the dispatch interval. A failure before submit leaves the later
    /// marks unset on the live request.
    fn prepare_native_prompt(
        &mut self,
        request: RequestId,
        waited: bool,
        steer: bool,
        launch: &str,
        terminal: &str,
        pane: &str,
    ) -> Result<(crate::messaging::model::BusState, String), String> {
        let mut state = self.state.clone();
        self.mark_open_dispatch(&mut state, request, DispatchMark::Started);
        // Messages that piled up while the agent was unavailable go as one prompt.
        let text = match waited.then(|| state.coalesce_queue(request)).flatten() {
            Some(text) => text,
            None => state
                .render_request_payload(request)
                .ok_or("Missing queued request")?,
        };
        self.mark_open_dispatch(&mut state, request, DispatchMark::Render);
        state.record_eligible(request, io::now_ms());
        let boundary = callbacks::boundary(&self.data_dir.join("callbacks").join(launch))
            .map_err(|e| e.to_string())?;
        self.mark_open_dispatch(&mut state, request, DispatchMark::Boundary);
        state
            .begin_submission(request, launch, boundary)
            .map_err(|e| e.to_string())?;
        state
            .stamp_delivery_epoch(
                request,
                crate::messaging::model::DeliveryEpoch {
                    generation: self.serving_generation,
                    terminal_id: terminal.to_owned(),
                    pane_id: pane.to_owned(),
                },
            )
            .map_err(|e| e.to_string())?;
        state
            .record_picked(request, io::now_ms(), waited || steer)
            .map_err(|e| e.to_string())?;
        state.begin_dialog_notice_write(request, io::now_ms());
        self.mark_open_dispatch(&mut state, request, DispatchMark::Notice);
        state
            .record_native_requested(request, io::now_ms())
            .map_err(|error| error.to_string())?;
        Ok((state, text))
    }

    fn deliver_request(
        &mut self,
        agent: &RoomAgent,
        waited: bool,
        steer: bool,
        allow_working: bool,
    ) -> Result<(), String> {
        let identity = &agent.runtime_identity;
        let (Some(launch), Some(terminal), Some(pane)) = (
            &identity.launch_id,
            &identity.terminal_id,
            &identity.pane_id,
        ) else {
            return Ok(());
        };
        if identity.session_id.is_none()
            && !matches!(
                agent.provider,
                Provider::Codex | Provider::Kimi | Provider::Agy
            )
        {
            return Ok(());
        }
        let Some(request) = self.state.next_queued_request(agent.id) else {
            return Ok(());
        };
        let _error_report = crate::utils::error_pipe::suppress(
            self.state
                .request(request)
                .is_some_and(super::Request::is_error_report),
        );
        let (state, text) =
            self.prepare_native_prompt(request, waited, steer, launch, terminal, pane)?;
        self.save(state)?;
        self.mark_live_dispatch(request, DispatchMark::Save);
        diagnostics::request(&self.state, request, "bus.delivery.start", "native_submit");
        self.trace_provider_retry(agent, request);
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
        self.mark_live_dispatch(request, DispatchMark::Diagnostics);
        let method = submission_method(
            agent,
            text,
            terminal,
            pane,
            steer,
            allow_working,
            self.serving_generation,
        );
        self.mark_live_dispatch(request, DispatchMark::Method);
        self.start_native_submission(agent, request, method, false, started)
    }

    pub(super) fn submission_outcome(
        result: Result<ResponseResult, TransportError>,
        session_id: &Option<String>,
    ) -> SubmissionOutcome {
        match result {
            Ok(ResponseResult::AgentPrompted { .. }) => SubmissionOutcome::Confirmed {
                provider_session_id: session_id.clone(),
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
        }
    }

    pub(super) fn note_withdrawn(
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
}
