use super::{
    Author, BusState, Draft, ModelError, Prompt, PromptId, Request, RequestId, RequestPhase, RoomId,
};

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
        self.submit_draft_from(room, Author::Developer, now_ms, false)
    }

    /// Sends the developer's draft to wait for each idle agent and its own turn.
    pub(crate) fn submit_draft_queued(
        &mut self,
        room: RoomId,
        now_ms: u64,
    ) -> Result<Vec<RequestId>, ModelError> {
        self.submit_draft_from(room, Author::Developer, now_ms, true)
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
            attention_origin: None,
        };
        let mut request_ids = Vec::with_capacity(prompt.recipient_ids.len());
        for agent_id in prompt.recipient_ids.iter().copied() {
            // End a previously healthy episode before this enqueue can cross
            // the pressure threshold again.
            let request_id = RequestId(self.allocate_id());
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
                    completed_at_ms: None,
                    queue_only,
                    group: None,
                    steered: false,
                    submitted_payload: None,
                    foreign_turn_settled_at_ms: None,
                    progress_at_ms: None,
                    turn_ended_at_ms: None,
                    awaiting_background: false,
                    shell_hold: None,
                },
            );
            self.queues.entry(agent_id).or_default().push(request_id);
            request_ids.push(request_id);
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
