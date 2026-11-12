use super::{
    Author, BusState, Draft, ModelError, Prompt, PromptId, Request, RequestId, RequestPhase, RoomId,
};
use crate::messaging::model::DeliveryLatency;

impl BusState {
    /// Queue an automation prompt without changing the room's developer-owned draft.
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
        let bus_generated = author == Author::Bus;
        self.enqueue_message(room, draft, author, now_ms, queue_only, bus_generated)
    }

    /// Bus-created inputs keep their visible author, but outlive a failed startup.
    pub(crate) fn submit_bus_message_with(
        &mut self,
        room: RoomId,
        draft: Draft,
        author: Author,
        now_ms: u64,
        queue_only: bool,
    ) -> Result<Vec<RequestId>, ModelError> {
        self.enqueue_message(room, draft, author, now_ms, queue_only, true)
    }

    fn enqueue_message(
        &mut self,
        room: RoomId,
        draft: Draft,
        author: Author,
        now_ms: u64,
        queue_only: bool,
        bus_generated: bool,
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
        let result = self.submit_draft_from(room, author, now_ms, queue_only, bus_generated);
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
        self.submit_draft_from(room, Author::Developer, now_ms, false, false)
    }

    /// Sends the developer's draft to wait for each idle agent and its own turn.
    pub(crate) fn submit_draft_queued(
        &mut self,
        room: RoomId,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        self.submit_draft_from(room, Author::Developer, now_ms, true, false)
    }

    pub(crate) fn submit_draft_exact(
        &mut self,
        room: RoomId,
        draft: Draft,
        now_ms: u64,
        queued: bool,
    ) -> Result<Vec<RequestId>, ModelError> {
        let original = std::mem::replace(
            &mut self
                .rooms
                .get_mut(&room)
                .ok_or(ModelError::UnknownRoom(room))?
                .draft,
            draft,
        );
        let result = self.submit_draft_from(room, Author::Developer, now_ms, queued, false);
        if result.is_err() {
            self.rooms
                .get_mut(&room)
                .ok_or(ModelError::UnknownRoom(room))?
                .draft = original;
        }
        result
    }

    fn submit_draft_from(
        &mut self,
        room: RoomId,
        author: Author,
        now_ms: u64,
        queue_only: bool,
        bus_generated: bool,
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
            if let Some(agent) = self.agents.get(agent_id) {
                if agent.deletion_pending {
                    return Err(ModelError::DeletionPending);
                }
                continue;
            }
            if let Some(retired) = self.retired_agents.get(agent_id) {
                return Err(ModelError::RecipientDeleted {
                    agent: *agent_id,
                    name: retired.name.clone(),
                });
            }
            return Err(ModelError::UnknownAgent(*agent_id));
        }

        let prompt = Prompt {
            id: PromptId(self.allocate_id()),
            author,
            text: draft.text,
            files: draft.files,
            recipient_ids: draft.recipient_ids,
            submitted_at_ms: now_ms,
            idle_agent_ids: Default::default(),
            attention_origin: None,
            in_reply_to: None,
        };
        let mut request_ids = Vec::with_capacity(prompt.recipient_ids.len());
        for agent_id in prompt.recipient_ids.iter().copied() {
            // End a previously healthy episode before this enqueue can cross
            // the pressure threshold again.
            let request_id = RequestId(self.allocate_id());
            // An ordinary agent request needs the orchestrator's own answer,
            // not the final of unrelated work already running.
            // Persist this per recipient so steering, coalescing and restart
            // retain the independent-turn boundary. Bus-generated notifications
            // keep their existing delivery policy.
            let independent = self.agent_request_queues(agent_id, &prompt.author, bus_generated);
            self.requests.insert(
                request_id,
                Request {
                    id: request_id,
                    room_id: room,
                    agent_id,
                    prompt: prompt.clone(),
                    sender_identity: self.message_sender(&prompt.author),
                    phase: RequestPhase::Queued,
                    failure_reason: None,
                    manual_recovery: None,
                    queued_rejection: None,
                    provider_retry_attempts: 0,
                    restart_continuation: false,
                    delivery_epoch: None,
                    expected_launch_id: None,
                    submission_boundary: None,
                    submission_status_revision: 0,
                    provider_session_id: None,
                    provider_turn_id: None,
                    provider_prompt_id: None,
                    trusted_start_bound: false,
                    uncertain_outcome: false,
                    submission_error: None,
                    dialog_delivery: None,
                    pending_final: None,
                    completed_reply: None,
                    completed_at_ms: None,
                    latency: DeliveryLatency {
                        accepted_at_ms: Some(now_ms),
                        // Developer and control sends fill this later in
                        // note_requests_queued. A bus-generated submit has no
                        // second writer, so queue admission is this instant.
                        queued_at_ms: bus_generated.then_some(now_ms),
                        ready_at_enqueue: self
                            .agents
                            .get(&agent_id)
                            .is_some_and(|agent| !agent.dialog && agent.wait_reason().is_none()),
                        ..Default::default()
                    },
                    queue_only: queue_only || independent,
                    bus_generated,
                    group: None,
                    steered: false,
                    submitted_payload: None,
                    foreign_turn_settled_at_ms: None,
                    progress_at_ms: None,
                    bound_turn_activity: None,
                    turn_ended_at_ms: None,
                    awaiting_background: false,
                },
            );
            self.queues.entry(agent_id).or_default().push(request_id);
            request_ids.push(request_id);
            self.abandon_failed_startup_queue(agent_id, now_ms);
        }
        let room_state = self
            .rooms
            .get_mut(&room)
            .ok_or(ModelError::UnknownRoom(room))?;
        // An agent's or Bus's message is news for the developer, like a reply;
        // their own is not.
        if prompt.author != Author::Developer && self.visible_room != Some(room) {
            room_state.unread_count = room_state.unread_count.saturating_add(1);
        }
        room_state.latest_prompt = Some(prompt);
        room_state.draft.text.clear();
        room_state.draft.files.clear();
        Ok(request_ids)
    }
}
