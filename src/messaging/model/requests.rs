//! Requests operations for the messaging state.
use super::types::clock;
use super::{
    AgentId, Author, BusState, Draft, ModelError, Prompt, PromptId, Request, RequestId,
    RequestPhase, RoomId, RuntimeStatus, SubmissionOutcome, BLOCKED_STALL_MS, DELIVERED_STALL_MS,
    QUEUED_STALL_MS,
};

impl BusState {
    /// Queue an automation prompt without changing the room's human-owned draft.
    pub(crate) fn submit_message_from(
        &mut self,
        room: RoomId,
        draft: Draft,
        author: Author,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        self.submit_message_with(room, draft, author, now_ms, false)
    }

    /// `queue_only` (`send --queue`) waits for an idle agent instead of
    /// steering a running turn, and is never coalesced with other messages.
    pub(crate) fn submit_message_with(
        &mut self,
        room: RoomId,
        draft: Draft,
        author: Author,
        now_ms: u64,
        queue_only: bool,
    ) -> Result<Vec<RequestId>, ModelError> {
        let original = self
            .rooms
            .get(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .draft
            .clone();
        self.rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .draft = draft;
        let result = self.submit_draft_from(room, author, now_ms, queue_only);
        self.rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?
            .draft = original;
        result
    }

    pub(crate) fn submit_draft(
        &mut self,
        room: RoomId,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        self.submit_draft_from(room, Author::Human, now_ms, false)
    }

    /// Sends the Human's draft to wait for each idle agent and its own turn.
    pub(crate) fn submit_draft_queued(
        &mut self,
        room: RoomId,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        self.submit_draft_from(room, Author::Human, now_ms, true)
    }

    fn submit_draft_from(
        &mut self,
        room: RoomId,
        author: Author,
        now_ms: u64,
        queue_only: bool,
    ) -> Result<Vec<RequestId>, ModelError> {
        let room_state = self.rooms.get(&room).ok_or(ModelError::UnknownRoom(room))?;
        if room_state.deletion_pending {
            return Err(ModelError::DeletionPending);
        }
        let draft = room_state.draft.clone();
        if draft.text.is_empty() && draft.files.is_empty() {
            return Err(ModelError::EmptyPrompt);
        }
        if draft.recipient_ids.is_empty() {
            return Err(ModelError::NoRecipients);
        }
        for agent_id in &draft.recipient_ids {
            let agent = self
                .agents
                .get(agent_id)
                .ok_or(ModelError::UnknownAgent(*agent_id))?;
            // A room's orchestrator lives in MASTER but takes messages from
            // the room it orchestrates, such as a worker saying it is blocked.
            if agent.room_id != room && agent.orchestrates != Some(room) {
                return Err(ModelError::AgentOutsideRoom(*agent_id));
            }
            if agent.deletion_pending {
                return Err(ModelError::DeletionPending);
            }
        }

        let prompt = Prompt {
            id: PromptId(self.allocate_id()),
            author,
            text: draft.text,
            files: draft.files,
            recipient_ids: draft.recipient_ids,
            submitted_at_ms: now_ms,
            compaction_limit_notice: false,
        };
        let mut request_ids = Vec::with_capacity(prompt.recipient_ids.len());
        for agent_id in prompt.recipient_ids.iter().copied() {
            let request_id = RequestId(self.allocate_id());
            self.requests.insert(
                request_id,
                Request {
                    id: request_id,
                    room_id: room,
                    agent_id,
                    prompt: prompt.clone(),
                    phase: RequestPhase::Queued,
                    failure_reason: None,
                    expected_launch_id: None,
                    submission_boundary: None,
                    submission_status_revision: 0,
                    provider_session_id: None,
                    provider_turn_id: None,
                    provider_prompt_id: None,
                    trusted_start_bound: false,
                    uncertain_outcome: false,
                    pending_final: None,
                    completed_at_ms: None,
                    queue_only,
                    group: None,
                    steered: false,
                    submitted_payload: None,
                    foreign_turn_settled_at_ms: None,
                    progress_at_ms: None,
                    turn_ended_at_ms: None,
                    awaiting_background: false,
                },
            );
            self.queues.entry(agent_id).or_default().push(request_id);
            request_ids.push(request_id);
        }
        let room_state = self
            .rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?;
        // Bus dialog notices are delivery-only (see `Request::delivery_only`).
        if prompt.author != Author::Bus {
            // An agent's message is news for the Human, like a reply; their own is not.
            if prompt.author != Author::Human && self.visible_room != Some(room) {
                room_state.unread_count = room_state.unread_count.saturating_add(1);
            }
            room_state.latest_prompt = Some(prompt);
        }
        room_state.draft.text.clear();
        room_state.draft.files.clear();
        Ok(request_ids)
    }

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
        request_state.expected_launch_id = Some(launch_id.to_owned());
        request_state.submission_boundary = Some(callback_boundary);
        request_state.submission_status_revision = submission_status_revision;
        request_state.foreign_turn_settled_at_ms = None;
        // Starts the idle clock for stall detection at the submission.
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
        match outcome {
            SubmissionOutcome::DefinitelyRejected { message } => {
                if request_state.trusted_start_bound {
                    return Err(ModelError::InvalidTransition);
                }
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
                agent.delivery_rejection = Some(rejection_reason(&message).into());
                agent.actionable_error = Some(message);
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
            }
            SubmissionOutcome::Uncertain { message } => {
                request_state.uncertain_outcome = true;
                self.agents
                    .get_mut(&agent_id)
                    .ok_or(ModelError::UnknownAgent(agent_id))?
                    .actionable_error = Some(message);
            }
        }
        Ok(())
    }

    /// Why `request` is stuck, once it has made no progress for a grace
    /// period, or `None`. A Working or Blocked agent is never stalled: a long
    /// turn is normal, and a blocked one waits on an answer, not on Bus (see
    /// `blocked_unanswered`). This only reports; the coordinator closes queued
    /// stalls before delivery, while typed work needs explicit recovery.
    pub(crate) fn stall_reason(&self, request: &Request, now_ms: u64) -> Option<String> {
        // A member typed into another request's turn shares that turn's fate.
        let lead = match request.group {
            Some(lead) => self.requests.get(&lead).unwrap_or(request),
            None => request,
        };
        let agent = self.agents.get(&lead.agent_id)?;
        let since = |grace: u64, from: u64| now_ms.saturating_sub(from) >= grace;
        if matches!(
            lead.phase,
            RequestPhase::Completed | RequestPhase::Abandoned
        ) {
            return None;
        }
        if matches!(
            agent.status,
            RuntimeStatus::Working | RuntimeStatus::Blocked
        ) {
            return None;
        }
        match lead.phase {
            RequestPhase::Completed | RequestPhase::Abandoned => None,
            RequestPhase::Queued => since(
                QUEUED_STALL_MS,
                lead.prompt.submitted_at_ms.max(agent.status_since_ms),
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
                if agent.status != RuntimeStatus::Idle
                    || !since(DELIVERED_STALL_MS, progress.max(agent.status_since_ms))
                {
                    return None;
                }
                Some(
                    if lead.phase == RequestPhase::Submitting {
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
        if matches!(
            request.phase,
            RequestPhase::Completed | RequestPhase::Abandoned
        ) {
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
    /// after Bus submitted the message, and has been seen Idle since. A captured reply also counts. Status
    /// transitions decide it, not reply capture, which can miss a reply.
    pub(crate) fn turn_ended(&self, request: &Request) -> bool {
        if request.phase == RequestPhase::Completed {
            return true;
        }
        // Submitting counts too: an uncertain submit stays Submitting until a
        // hook binds it, while the agent may already be working on it.
        if !matches!(
            request.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) {
            return false;
        }
        self.agents.get(&request.agent_id).is_some_and(|agent| {
            request.turn_ended_at_ms.is_some()
                || (agent.busy_revision > request.submission_status_revision
                    && agent.status == RuntimeStatus::Idle
                    && agent.status_revision > agent.busy_revision)
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

    /// The request a message may steer into: the agent's current request while
    /// its turn visibly runs, bound by its submit hook, with no final reply yet.
    /// Returns the first queued request and that lead, unless the queued one
    /// asked to wait (`--queue`).
    pub(crate) fn next_steering(&self, agent: AgentId) -> Option<(RequestId, RequestId)> {
        let agent_state = self.agents.get(&agent)?;
        if agent_state.status != RuntimeStatus::Working
            || agent_state.dialog
            || agent_state.deletion_pending
        {
            return None;
        }
        let lead = self.requests.get(&agent_state.current_request?)?;
        if lead.phase != RequestPhase::Active
            || !lead.trusted_start_bound
            || lead.pending_final.is_some()
            || lead.turn_ended_at_ms.is_some()
        {
            return None;
        }
        let first = *self.queues.get(&agent)?.first()?;
        (!self.requests.get(&first)?.queue_only).then_some((first, lead.id))
    }

    /// Moves `request` from the queue into `lead`'s group before Bus types it
    /// into the running turn. The lead then settles only on an idle status
    /// observed after this, so the turn the typing extends is not cut short.
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
        if agent.current_request != Some(lead)
            || self.queues.get(&agent_id).and_then(|queue| queue.first()) != Some(&request)
        {
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
            queue.remove(0);
        }
        let request_state = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        request_state.phase = RequestPhase::Submitting;
        request_state.expected_launch_id = launch;
        request_state.group = Some(lead);
        request_state.steered = true;
        if let Some(lead) = self.requests.get_mut(&lead) {
            lead.submission_status_revision = status_revision;
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
        match outcome {
            SubmissionOutcome::Confirmed { .. } => request_state.phase = RequestPhase::Active,
            SubmissionOutcome::Uncertain { .. } => {
                request_state.phase = RequestPhase::Active;
                request_state.uncertain_outcome = true;
            }
            SubmissionOutcome::DefinitelyRejected { .. } => {
                request_state.phase = RequestPhase::Queued;
                request_state.expected_launch_id = None;
                request_state.group = None;
                request_state.steered = false;
                let agent_id = request_state.agent_id;
                self.queues.entry(agent_id).or_default().insert(0, request);
            }
        }
        Ok(())
    }

    /// Joins the queued messages that piled up while the agent could not take
    /// them into `lead`'s prompt: consecutive ones from the queue front, up to
    /// the first sent with `--queue`. Returns the text to type, each part
    /// marked with its sender and time, or `None` when `lead` goes alone.
    pub(crate) fn coalesce_queue(&mut self, lead: RequestId) -> Option<String> {
        let agent_id = self.requests.get(&lead)?.agent_id;
        let queue = self.queues.get(&agent_id)?;
        if queue.first() != Some(&lead) || self.requests.get(&lead)?.queue_only {
            return None;
        }
        let members: Vec<RequestId> = queue[1..]
            .iter()
            .copied()
            .take_while(|id| self.requests.get(id).is_some_and(|r| !r.queue_only))
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
                let prompt = &self.requests.get(id)?.prompt;
                Some(format!(
                    "[{}/{count} from {} at {}]\n{}",
                    index + 1,
                    self.sender_name(&prompt.author),
                    clock(prompt.submitted_at_ms),
                    prompt.rendered_payload()
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
            Author::Human => "the human".into(),
            Author::Bus => "Bus".into(),
            Author::Agent(id) => self
                .agents
                .get(id)
                .map_or_else(|| "an agent".into(), |agent| agent.name.clone()),
        }
    }

    /// The requests that joined `lead`'s group, oldest first.
    pub(crate) fn group_members(&self, lead: RequestId) -> Vec<RequestId> {
        self.requests
            .values()
            .filter(|request| request.group == Some(lead))
            .map(|request| request.id)
            .collect()
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

/// The stall reason for a native refusal to type a request. The native
/// server (`src/server/api/agents/prompt.rs`) shares one `agent_not_ready` code for
/// several causes, so the message tells them apart.
pub(crate) fn rejection_reason(message: &str) -> &'static str {
    if message.contains("input box is not empty") || message.contains("composer is not empty") {
        "input_box_not_empty"
    } else if message.contains("blocked") {
        "agent_blocked"
    } else if message.contains("not ready") || message.contains("not an active") {
        "agent_not_ready"
    } else {
        "delivery_rejected"
    }
}
