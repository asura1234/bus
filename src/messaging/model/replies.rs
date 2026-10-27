//! Recipient-authored answers settle one request without closing its provider turn.
use super::{
    types::CompletedReply, AgentId, BusState, PendingFinal, PromptId, Reply, RequestId,
    RequestPhase,
};

impl BusState {
    pub(crate) fn answer_message(
        &mut self,
        message: PromptId,
        recipient: AgentId,
        text: String,
        now_ms: u64,
    ) -> Result<RequestId, String> {
        let (id, room, content) = {
            let mut requests = self.requests_for_message(message).peekable();
            if requests.peek().is_none() {
                return Err("Unknown reply-to message ID".into());
            }
            let request = requests
                .find(|request| request.agent_id == recipient)
                .ok_or("The replying agent is not a recipient of this message")?;
            let id = request.id;
            if let Some(reply) = request.reply() {
                return if reply.text == text {
                    Ok(id)
                } else {
                    Err("This request already has a different reply".into())
                };
            }
            if !matches!(
                request.phase,
                RequestPhase::Submitting | RequestPhase::Active
            ) {
                return Err("The request is not awaiting a recipient answer".into());
            }
            if text.trim().is_empty() {
                return Err("A reply must contain answer text".into());
            }
            let room = request.room_id;
            let content = PendingFinal {
                callback_id: format!("reply-to:{}", id.0),
                text: text.clone(),
                received_at_ms: now_ms,
                provider_session_id: request.provider_session_id.clone(),
                provider_turn_id: request.provider_turn_id.clone(),
            };
            (id, room, content)
        };
        // The anchor's phase, binding and pending turn final deliberately remain intact.
        let request = self
            .requests
            .get_mut(&id)
            .ok_or("Request is no longer active")?;
        request.completed_reply = Some(CompletedReply {
            content,
            group: None,
        });
        request.completed_at_ms = Some(now_ms);
        let room = self
            .rooms
            .get_mut(&room)
            .ok_or("Original conversation no longer exists")?;
        room.latest_replies.insert(
            recipient,
            Reply {
                request_id: id,
                agent_id: recipient,
                text,
                received_at_ms: now_ms,
            },
        );
        if self.visible_room != Some(room.id) {
            room.unread_count = room.unread_count.saturating_add(1);
        }
        Ok(id)
    }
}
