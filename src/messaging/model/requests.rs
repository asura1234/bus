//! Requests operations for the messaging state.
use super::types::clock;
use super::{
    AgentId, Author, BusState, Draft, ModelError, Prompt, PromptId, Request, RequestId,
    RequestPhase, RoomId, RuntimeStatus, SubmissionOutcome, BLOCKED_STALL_MS, BUS_SENDER,
    DELIVERED_STALL_MS, QUEUED_STALL_MS,
};

mod rejection;
mod submit;

impl BusState {
    pub(crate) fn begin_submission(
        &mut self,
        request: RequestId,
        launch_id: &str,
        callback_boundary: u64,
    ) -> Result<(), ModelError> {
        let request_state = self
            .requests
            .get(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if request_state.phase != RequestPhase::Queued {
            return Err(ModelError::InvalidTransition);
        }
        let agent_id = request_state.agent_id;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        if agent.deletion_pending {
            return Err(ModelError::DeletionPending);
        }
        let expected_launch = agent
            .runtime_identity
            .launch_id
            .as_deref()
            .ok_or(ModelError::MissingLaunchIdentity)?;
        if expected_launch != launch_id {
            return Err(ModelError::LaunchIdentityMismatch);
        }
        let submission_status_revision = agent.status_revision;
        let submitted_at_ms = agent.observed_at_ms;
        if agent.current_request.is_some()
            || self
                .queues
                .get(&agent_id)
                .and_then(|queue| queue.first())
                .copied()
                != Some(request)
        {
            return Err(ModelError::InvalidTransition);
        }

        let queue = self
            .queues
            .get_mut(&agent_id)
            .ok_or(ModelError::InvalidTransition)?;
        queue.remove(0);
        self.agents
            .get_mut(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?
            .current_request = Some(request);
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        request_state.phase = RequestPhase::Submitting;
        request_state.submission_error = None;
        request_state.expected_launch_id = Some(launch_id.to_owned());
        request_state.submission_boundary = Some(callback_boundary);
        request_state.submission_status_revision = submission_status_revision;
        request_state.foreign_turn_settled_at_ms = None;
        // Starts the idle recovery clock at the submission.
        request_state.progress_at_ms = Some(submitted_at_ms);
        Ok(())
    }

    pub(crate) fn record_submission(
        &mut self,
        request: RequestId,
        outcome: SubmissionOutcome,
    ) -> Result<(), ModelError> {
        let agent_id = self
            .requests
            .get(&request)
            .ok_or(ModelError::UnknownRequest(request))?
            .agent_id;
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if request_state.phase != RequestPhase::Submitting {
            return Err(ModelError::InvalidTransition);
        }
        if matches!(
            &outcome,
            SubmissionOutcome::DefinitelyRejected { .. } | SubmissionOutcome::ComposerBusy
        ) && request_state.trusted_start_bound
        {
            return Err(ModelError::InvalidTransition);
        }
        self.record_queue_rejection(request, &outcome);
        self.record_dialog_notice_write(request, &outcome);
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        let rejection = match &outcome {
            SubmissionOutcome::DefinitelyRejected { message } => Some(message.clone()),
            _ => None,
        };
        match outcome {
            SubmissionOutcome::DefinitelyRejected { .. } | SubmissionOutcome::ComposerBusy => {
                request_state.phase = RequestPhase::Queued;
                request_state.expected_launch_id = None;
                request_state.submission_boundary = None;
                request_state.submitted_payload = None;
                // Coalesced members go back behind their lead, in order.
                let members = self.group_members(request);
                for member in &members {
                    if let Some(member) = self.requests.get_mut(member) {
                        member.phase = RequestPhase::Queued;
                        member.group = None;
                    }
                }
                let queue = self.queues.entry(agent_id).or_default();
                queue.splice(0..0, std::iter::once(request).chain(members));
                let agent = self
                    .agents
                    .get_mut(&agent_id)
                    .ok_or(ModelError::UnknownAgent(agent_id))?;
                agent.current_request = None;
                agent.delivery_rejection = Some(
                    rejection
                        .as_deref()
                        .map_or("composer_busy", rejection_reason)
                        .into(),
                );
                agent.actionable_error = rejection;
            }
            SubmissionOutcome::Confirmed {
                provider_session_id,
                provider_turn_id,
            } => {
                if let Some(agent) = self.agents.get_mut(&agent_id) {
                    agent.delivery_rejection = None;
                }
                let Some(request_state) = self.requests.get_mut(&request) else {
                    return Err(ModelError::UnknownRequest(request));
                };
                request_state.phase = RequestPhase::Active;
                request_state.provider_session_id = provider_session_id;
                request_state.provider_turn_id = provider_turn_id;
                request_state.uncertain_outcome = false;
                request_state.submission_error = None;
            }
            SubmissionOutcome::Uncertain { message } => {
                request_state.uncertain_outcome = true;
                request_state.submission_error = Some(message.clone());
                let agent = self
                    .agents
                    .get_mut(&agent_id)
                    .ok_or(ModelError::UnknownAgent(agent_id))?;
                if agent.delivery_rejection.as_deref() == Some("composer_busy") {
                    agent.delivery_rejection = None;
                }
                agent.actionable_error = Some(message);
            }
        }
        Ok(())
    }

    /// When the agent last worked on a request it took: a turn event, or that
    /// request settling. A turn that starts and ends between two status polls
    /// leaves no status change, so the queue behind it counts from this too.
    /// A refused paste returns its request to the queue and is no progress.
    fn last_progress_ms(&self, agent: AgentId) -> u64 {
        self.requests
            .values()
            .filter(|request| request.agent_id == agent && request.phase != RequestPhase::Queued)
            .filter_map(|request| {
                let progress = request.progress_at_ms?;
                Some(progress.max(request.completed_at_ms.unwrap_or(0)))
            })
            .max()
            .unwrap_or(0)
    }

    /// Why `request` needs recovery once it has made no progress for a grace
    /// period, or `None`. Unattempted work waits through Working/Blocked:
    /// a long turn is normal, and a blocked one waits on an answer (see
    /// `blocked_unanswered`). The coordinator closes expired queued requests
    /// before delivery, while typed work needs explicit recovery.
    /// Definitely rejected queued work has its own fixed deadline.
    pub(crate) fn recovery_reason(&self, request: &Request, now_ms: u64) -> Option<String> {
        if request.settled() {
            return None;
        }
        // A member typed into another request's turn shares that turn's fate.
        let lead = match request.group {
            Some(lead) => self.requests.get(&lead).unwrap_or(request),
            None => request,
        };
        let agent = self.agents.get(&lead.agent_id)?;
        if let Some(reason) = self.expired_queue_rejection(lead, now_ms) {
            return Some(reason.into());
        }
        let since = |grace: u64, from: u64| now_ms.saturating_sub(from) >= grace;
        if matches!(
            lead.phase,
            RequestPhase::Completed | RequestPhase::Abandoned
        ) {
            return None;
        }
        // A visible dialog blocks the agent even while Codex reports Idle.
        if matches!(
            agent.shown_status(),
            RuntimeStatus::Working | RuntimeStatus::Blocked
        ) {
            return None;
        }
        match lead.phase {
            RequestPhase::Completed | RequestPhase::Abandoned => None,
            // This is a deliberate wait on developer-owned input, not a failed
            // delivery. Persisting the reason also protects the queue on restart.
            RequestPhase::Queued
                if agent.delivery_rejection.as_deref() == Some("composer_busy") =>
            {
                None
            }
            // A recoverable startup wait has no submission opportunity.
            // Conclusive failures close their queue when recorded or enqueued.
            RequestPhase::Queued
                if agent
                    .startup
                    .as_ref()
                    .is_some_and(|s| s.reported || s.recovery.is_some()) =>
            {
                None
            }
            // An unconfirmed write ahead of it is failed at its own bound
            // (`fail_unconfirmed_submissions`); until then nothing refuses it.
            RequestPhase::Queued
                if agent.status == RuntimeStatus::Idle
                    && agent
                        .current_request
                        .and_then(|id| self.requests.get(&id))
                        .is_some_and(|current| current.phase == RequestPhase::Submitting) =>
            {
                None
            }
            RequestPhase::Queued => since(
                QUEUED_STALL_MS,
                lead.prompt
                    .submitted_at_ms
                    .max(agent.status_since_ms)
                    .max(self.last_progress_ms(lead.agent_id)),
            )
            .then(|| {
                agent
                    .delivery_rejection
                    .clone()
                    .or_else(|| agent.wait_reason().map(Into::into))
                    .unwrap_or_else(|| "not_submitted".into())
            }),
            RequestPhase::Submitting | RequestPhase::Active => {
                let progress = lead.progress_at_ms.unwrap_or(agent.status_since_ms);
                let invalidated =
                    agent.status == RuntimeStatus::Unavailable && agent.session_binding_invalidated;
                if (agent.status != RuntimeStatus::Idle && !invalidated)
                    || !since(DELIVERED_STALL_MS, progress.max(agent.status_since_ms))
                {
                    return None;
                }
                if agent.provider == super::Provider::Kimi
                    && lead.trusted_start_bound
                    && !invalidated
                {
                    return None;
                }
                Some(
                    if invalidated {
                        "session_invalidated"
                    } else if lead.phase == RequestPhase::Submitting {
                        "submission_unconfirmed"
                    } else if !lead.trusted_start_bound {
                        "no_start_hook"
                    } else {
                        // The turn started and the agent is idle, yet no
                        // final reply matched the provider's transcript.
                        "transcript_unmatched"
                    }
                    .into(),
                )
            }
        }
    }

    /// Whether `request`'s agent has sat blocked (a dialog, question or trust
    /// prompt) past `BLOCKED_STALL_MS` while the request is unsettled. The
    /// agent asks for help once per blocked episode; this surfaces a block
    /// nobody answered in `message status`, without ending `send --async`,
    /// which waits on through a block by design.
    pub(crate) fn blocked_unanswered(&self, request: &Request, now_ms: u64) -> bool {
        if request.settled() {
            return false;
        }
        self.agents.get(&request.agent_id).is_some_and(|agent| {
            let from = agent
                .status_since_ms
                .max(request.progress_at_ms.unwrap_or(0))
                .max(request.prompt.submitted_at_ms);
            agent.status == RuntimeStatus::Blocked
                && now_ms.saturating_sub(from) >= BLOCKED_STALL_MS
        })
    }

    /// Whether `request`'s agent took the message and finished its turn: the
    /// agent was seen Working or Blocked, or its provider reported the turn,
    /// after Bus submitted the message, and has been seen Idle since. A captured provider final also counts. Status
    /// transitions decide it, not reply capture, which can miss a reply.
    pub(crate) fn turn_ended(&self, request: &Request) -> bool {
        if request.phase == RequestPhase::Completed {
            return true;
        }
        // Kimi can briefly paint an empty, idle composer while its bound
        // turn is still executing tools. Only its owned terminal wire (or a
        // captured reply) can establish that the turn actually finished.
        if self
            .agents
            .get(&request.agent_id)
            .is_some_and(|agent| agent.provider == super::Provider::Kimi)
        {
            return false;
        }
        // An uncertain write without a matching provider start may be a
        // startup dialog or unrelated activity, not work on this message.
        if request.uncertain_outcome && !request.trusted_start_bound {
            return false;
        }
        // Background work without a captured answer will wake
        // the turn with the real reply, for its joined messages too.
        let lead = request
            .group
            .and_then(|lead| self.requests.get(&lead))
            .unwrap_or(request);
        if lead.awaiting_background {
            return false;
        }
        // Screen activity alone cannot confirm an uncertain submission;
        // a confirmed write may finish without a captured start hook.
        if !matches!(
            request.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) {
            return false;
        }
        // A visible dialog (Codex reports Idle while it waits on an approval)
        // means the turn is waiting on an answer, not finished.
        self.agents.get(&request.agent_id).is_some_and(|agent| {
            !agent.dialog
                && (request.turn_ended_at_ms.is_some()
                    || (agent.busy_revision > request.submission_status_revision
                        && agent.status == RuntimeStatus::Idle
                        && agent.status_revision > agent.busy_revision))
        })
    }

    /// The agent's current request when Bus typed it but never saw it start:
    /// typed, never bound by a submit hook, and with no reply yet.
    pub(super) fn unbound_request(&self, agent: AgentId) -> Option<&Request> {
        let request = self
            .requests
            .get(&self.agents.get(&agent)?.current_request?)?;
        // An uncertain submit outcome stays Submitting until a hook binds it.
        (matches!(
            request.phase,
            RequestPhase::Active | RequestPhase::Submitting
        ) && !request.steered
            && !request.trusted_start_bound
            && request.pending_final.is_none())
        .then_some(request)
    }

    /// Releases the agent's queue from a request it never started as a turn of
    /// its own, once the agent proved it moved on: it finished a turn of its
    /// own and then started another, began a new provider session, or went
    /// idle. The typed text may have joined the agent's own turn, so its reply,
    /// if any, is in the terminal. Bus says nothing: it never authors a message,
    /// and a waiting sender sees the request `abandoned`.
    pub(crate) fn release_unbound_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
    ) -> Option<RequestId> {
        let request = self.unbound_request(agent)?.id;
        self.abandon_current_request(request, now_ms).ok()?;
        tracing::info!(
            event = "bus.message.recovered",
            request_id = request.0,
            agent_id = agent.0,
            reason = "never_started",
            "Unstarted request released"
        );
        Some(request)
    }

    /// Bounds waiting for an unconfirmed write with no observed start hook:
    /// fail it after `DELIVERED_STALL_MS` from its latest progress or idle
    /// start, while its agent remains idle. This does not prove the provider
    /// never ran it. Tell the developer its outcome is unknown, never retype
    /// it, and let the queue move on. Any text still in the composer is
    /// cleared by the next delivery.
    pub(crate) fn fail_unconfirmed_submissions(&mut self, now_ms: u64) -> Result<(), ModelError> {
        let unconfirmed: Vec<_> = self
            .agents
            .keys()
            .filter_map(|agent| self.unbound_request(*agent))
            .filter(|request| request.phase == RequestPhase::Submitting)
            .filter(|request| {
                self.recovery_reason(request, now_ms).as_deref() == Some("submission_unconfirmed")
            })
            .map(|request| (request.id, request.agent_id, request.prompt.id))
            .collect();
        for (request, agent, prompt) in unconfirmed {
            let message = self
                .requests
                .get(&request)
                .and_then(Request::submission_notice)
                .unwrap_or_else(|| Self::unconfirmed_submission_message(prompt));
            self.close_turn_with_reason(request, now_ms, "submission_unconfirmed".into())?;
            self.set_agent_error(agent, Some(message))?;
            tracing::info!(
                event = "bus.message.recovered",
                request_id = request.0,
                agent_id = agent.0,
                reason = "submission_unconfirmed",
                "Unconfirmed submission failed"
            );
        }
        Ok(())
    }

    /// The request a message may steer into: the agent's current request while
    /// its turn visibly runs, or while the idle agent has no answer yet and
    /// waits for background work that turn started (shells, agents, crons),
    /// bound by its submit hook, with no final reply yet. Only the lead's own
    /// room can steer ordinary work; blocked-dialog notices may interrupt any
    /// room's turn, including when older cross-room work heads the queue.
    pub(crate) fn next_steering(&self, agent: AgentId) -> Option<(RequestId, RequestId)> {
        let agent_state = self.agents.get(&agent)?;
        if !matches!(
            agent_state.status,
            RuntimeStatus::Working | RuntimeStatus::Idle
        ) || agent_state.dialog
            || agent_state.deletion_pending
            // A pressed stop key ends the turn: its queue closes with it.
            || agent_state.interrupt_pending
        {
            return None;
        }
        let lead = self.requests.get(&agent_state.current_request?)?;
        if (agent_state.status == RuntimeStatus::Idle && !lead.awaiting_background)
            || lead.phase != RequestPhase::Active
            || !lead.trusted_start_bound
            || lead.pending_final.is_some()
            || lead.turn_ended_at_ms.is_some()
        {
            return None;
        }
        let queue = self.queues.get(&agent)?;
        if let Some(notice) = queue.iter().find(|id| self.is_dialog_notice(**id)) {
            return Some((*notice, lead.id));
        }
        let first = *queue.first()?;
        let queued = self.requests.get(&first)?;
        // Grok 1.0.70 shows a follow-up typed during a turn as Enter:queue and
        // does not accept it before the submit deadline. Withdrawal then
        // refuses that composer, so the draft is still there after the turn
        // goes idle. Hold ordinary work for a normal submit once the turn ends.
        // A blocked-dialog notice above may still interrupt the turn.
        if agent_state.provider == super::Provider::Grok
            && agent_state.status == RuntimeStatus::Working
        {
            return None;
        }
        (!queued.queue_only && queued.room_id == lead.room_id).then_some((first, lead.id))
    }

    /// Moves `request` from the queue into `lead`'s group before Bus types it
    /// into the running turn. A captured answer settles the members that its
    /// hook or transcript claims; an unclaimed input retains its uncertainty.
    pub(crate) fn begin_steering(
        &mut self,
        request: RequestId,
        lead: RequestId,
    ) -> Result<(), ModelError> {
        let agent_id = self
            .requests
            .get(&request)
            .ok_or(ModelError::UnknownRequest(request))?
            .agent_id;
        let agent = self
            .agents
            .get(&agent_id)
            .ok_or(ModelError::UnknownAgent(agent_id))?;
        if self.next_steering(agent_id) != Some((request, lead)) {
            return Err(ModelError::InvalidTransition);
        }
        let status_revision = agent.status_revision;
        let launch = self
            .requests
            .get(&lead)
            .ok_or(ModelError::UnknownRequest(lead))?
            .expected_launch_id
            .clone();
        if let Some(queue) = self.queues.get_mut(&agent_id) {
            queue.retain(|id| *id != request);
        }
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        request_state.phase = RequestPhase::Submitting;
        request_state.expected_launch_id = launch;
        request_state.group = Some(lead);
        request_state.steered = true;
        // Until the native write is confirmed, the member keeps the lead's
        // previous boundary so a definite rejection can restore it.
        if let Some(lead) = self.requests.get_mut(&lead) {
            let previous = std::mem::replace(&mut lead.submission_status_revision, status_revision);
            if let Some(member) = self.requests.get_mut(&request) {
                member.submission_status_revision = previous;
            }
        }
        Ok(())
    }

    /// Records the native outcome of typing a steering message. A definite
    /// rejection (the agent stopped working) returns it to the queue front.
    pub(crate) fn record_steering(
        &mut self,
        request: RequestId,
        outcome: SubmissionOutcome,
    ) -> Result<(), ModelError> {
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if request_state.phase != RequestPhase::Submitting || !request_state.steered {
            return Err(ModelError::InvalidTransition);
        }
        let agent_id = request_state.agent_id;
        self.record_queue_rejection(request, &outcome);
        self.record_dialog_notice_write(request, &outcome);
        let composer_busy = outcome == SubmissionOutcome::ComposerBusy;
        if let Some(agent) = self.agents.get_mut(&agent_id).filter(|agent| {
            !composer_busy && agent.delivery_rejection.as_deref() == Some("composer_busy")
        }) {
            agent.delivery_rejection = None;
        }
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        match outcome {
            SubmissionOutcome::Confirmed { .. } => request_state.phase = RequestPhase::Active,
            SubmissionOutcome::Uncertain { .. } => {
                request_state.phase = RequestPhase::Active;
                request_state.uncertain_outcome = true;
            }
            SubmissionOutcome::DefinitelyRejected { .. } | SubmissionOutcome::ComposerBusy => {
                // Nothing was typed, so the lead's turn keeps the work it
                // was already seen doing.
                let lead = request_state.group;
                let previous = request_state.submission_status_revision;
                request_state.phase = RequestPhase::Queued;
                request_state.expected_launch_id = None;
                request_state.group = None;
                request_state.steered = false;
                let agent_id = request_state.agent_id;
                self.queues.entry(agent_id).or_default().insert(0, request);
                if let Some(lead) = lead.and_then(|lead| self.requests.get_mut(&lead)) {
                    lead.submission_status_revision = previous;
                }
                if let Some(agent) = self.agents.get_mut(&agent_id).filter(|_| composer_busy) {
                    agent.delivery_rejection = Some("composer_busy".into());
                    agent.actionable_error = None;
                }
            }
        }
        Ok(())
    }

    /// Joins the queued messages that piled up while the agent could not take
    /// them into `lead`'s prompt: consecutive ones from the queue front, up to
    /// the first sent with `--queue` or from another room. Returns the text to type, each part
    /// marked with its sender and time, or `None` when `lead` goes alone.
    pub(crate) fn coalesce_queue(&mut self, lead: RequestId) -> Option<String> {
        let request = self.requests.get(&lead)?;
        let (agent_id, room) = (request.agent_id, request.room_id);
        let queue = self.queues.get(&agent_id)?;
        if queue.first() != Some(&lead) || self.requests.get(&lead)?.queue_only {
            return None;
        }
        let members: Vec<RequestId> = queue[1..]
            .iter()
            .copied()
            .take_while(|id| {
                self.requests
                    .get(id)
                    .is_some_and(|r| !r.queue_only && r.room_id == room)
            })
            .collect();
        if members.is_empty() {
            return None;
        }
        let parts: Vec<RequestId> = std::iter::once(lead)
            .chain(members.iter().copied())
            .collect();
        let count = parts.len();
        let text = parts
            .iter()
            .enumerate()
            .filter_map(|(index, id)| {
                let payload = self.render_request_payload(*id)?;
                let prompt = &self.requests.get(id)?.prompt;
                let attribution = match prompt.author {
                    Author::Agent(_) => String::new(),
                    _ => format!(" from {}", self.sender_name(&prompt.author)),
                };
                Some(format!(
                    "[{}/{count}{attribution} at {}]\n{}",
                    index + 1,
                    clock(prompt.submitted_at_ms),
                    payload
                ))
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if let Some(queue) = self.queues.get_mut(&agent_id) {
            queue.retain(|id| !members.contains(id));
        }
        for member in members {
            if let Some(request) = self.requests.get_mut(&member) {
                request.phase = RequestPhase::Active;
                request.group = Some(lead);
            }
        }
        if let Some(request) = self.requests.get_mut(&lead) {
            request.submitted_payload = Some(text.clone());
        }
        Some(text)
    }

    fn sender_name(&self, author: &Author) -> String {
        match author {
            Author::Developer => "the developer".into(),
            Author::Bus => BUS_SENDER.into(),
            Author::Agent(id) => self
                .agents
                .get(id)
                .map_or_else(|| "an agent".into(), |agent| agent.name.clone()),
        }
    }

    /// The requests that joined `lead`'s group, oldest first.
    pub(crate) fn group_members(&self, lead: RequestId) -> Vec<RequestId> {
        // Archived groups too: the room history shows a group's reply once.
        self.requests()
            .filter(|request| request.group == Some(lead))
            .map(|request| request.id)
            .collect()
    }

    /// Every group's members by lead, oldest first: `group_members` for all
    /// leads in one pass, for views that place every group at once.
    pub(crate) fn groups(&self) -> std::collections::BTreeMap<RequestId, Vec<RequestId>> {
        let mut groups = std::collections::BTreeMap::<RequestId, Vec<RequestId>>::new();
        for request in self.requests() {
            let lead = request.reply_key();
            if lead != request.id {
                groups.entry(lead).or_default().push(request.id);
            }
        }
        groups
    }

    /// Real blocked-worker notices take precedence in an orchestrator's inbox.
    /// Use the persisted dialog routing identity, never sender-authored text.
    /// Keep FIFO order within each class. Idle delivery gives each notice its
    /// own turn; `next_steering` allows it into an already-active turn.
    pub(crate) fn prioritize_orchestrator_dialogs(&mut self, agent: AgentId) -> bool {
        if self
            .agents
            .get(&agent)
            .is_none_or(|a| a.orchestrates.is_none())
        {
            return false;
        }
        let notices: std::collections::BTreeSet<_> = self
            .queued_requests(agent)
            .iter()
            .copied()
            .filter(|id| self.is_dialog_notice(*id))
            .collect();
        let Some(queue) = self.queues.get_mut(&agent) else {
            return false;
        };
        let before = queue.clone();
        queue.sort_by_key(|id| !notices.contains(id));
        let mut changed = *queue != before;
        for id in queue.iter().filter(|id| notices.contains(id)) {
            if let Some(request) = self.requests.get_mut(id) {
                changed |= !request.queue_only;
                request.queue_only = true;
            }
        }
        changed
    }

    pub(crate) fn next_queued_request(&self, agent: AgentId) -> Option<RequestId> {
        let agent_state = self.agents.get(&agent)?;
        if agent_state.current_request.is_some() || agent_state.deletion_pending {
            return None;
        }
        self.queues.get(&agent)?.first().copied()
    }

    pub(crate) fn queued_requests(&self, agent: AgentId) -> &[RequestId] {
        self.queues.get(&agent).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// The recovery reason for a native refusal to type a request. The native
/// server (`src/server/api/agents/prompt.rs`) shares one `agent_not_ready` code for
/// several causes, so the message tells them apart.
pub(crate) fn rejection_reason(message: &str) -> &'static str {
    if message.contains("input box is not empty") || message.contains("composer is not empty") {
        "input_box_not_empty"
    } else if message.contains(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE) {
        "prompt_not_shown"
    } else if message.contains("blocked") {
        "agent_blocked"
    } else if message.contains("not ready") || message.contains("not an active") {
        "agent_not_ready"
    } else {
        "delivery_rejected"
    }
}

impl Request {
    /// Durable send outcome, never the agent's mutable latest error or a reply.
    pub(crate) fn submission_notice(&self) -> Option<String> {
        if self.failure_reason.as_deref() == Some(super::AGENT_STARTUP_FAILED) {
            return self.submission_error.clone();
        }
        let unconfirmed = self.failure_reason.as_deref() == Some("submission_unconfirmed")
            || self.phase == RequestPhase::Submitting
                && self.uncertain_outcome
                && !self.trusted_start_bound;
        if !unconfirmed {
            return None;
        }
        let guidance = BusState::unconfirmed_submission_message(self.prompt.id);
        Some(match &self.submission_error {
            Some(detail) => format!("{guidance}\n{detail}"),
            None => guidance,
        })
    }
}
