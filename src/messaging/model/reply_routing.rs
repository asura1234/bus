//! Durable routing of a worker's captured reply back to its room orchestrator.
use super::{
    AgentId, Author, BusState, Draft, ModelError, PromptId, Request, RequestId, RequestPhase,
};
use std::collections::{BTreeMap, BTreeSet};

const REPLY_GRACE_MS: u64 = 2_000;
const INLINE_REPLY_CHARS: usize = 4_000;

impl BusState {
    pub(crate) fn reply_orchestrator(&self, request: &Request) -> Option<AgentId> {
        let worker = self.agent(request.agent_id)?;
        let room = self.room(request.room_id)?;
        let orchestrator = self.orchestrator_of(room.id)?;
        (request.prompt.author == Author::Agent(orchestrator.id)
            && worker.room_id == room.id
            && worker.id != orchestrator.id
            && !worker.deletion_pending
            && !orchestrator.deletion_pending
            && !room.deletion_pending)
            .then_some(orchestrator.id)
    }

    pub(crate) fn reply_needs_ack(&self, request: &Request) -> bool {
        request.phase == RequestPhase::Completed
            && request.pending_final.is_some()
            && self.reply_orchestrator(request).is_some()
            && self.reply_routes.get(&reply_key(request)) != Some(&None)
    }

    pub(crate) fn is_reply_notification(&self, request: RequestId) -> bool {
        self.reply_routes
            .values()
            .any(|notice| *notice == Some(request))
    }

    pub(crate) fn reply_delivery_pending(&self, request: &Request) -> bool {
        // Absence is the durable "not yet routed/seen" state. A recovered
        // callback may be older than retention before its first route pass.
        if !self.reply_routes.contains_key(&reply_key(request)) {
            return self.reply_needs_ack(request);
        }
        self.reply_routes
            .get(&reply_key(request))
            .copied()
            .flatten()
            .and_then(|id| self.request(id))
            .is_some_and(|notice| !notice.settled())
    }

    /// A status read never marks a reply seen: only a waiter that already
    /// flushed the reply and names its original author may suppress the wake.
    pub(crate) fn see_worker_reply(
        &mut self,
        request: RequestId,
        author: AgentId,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let source = self
            .request(request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if self.reply_orchestrator(source) != Some(author)
            || source.phase != RequestPhase::Completed
            || source.pending_final.is_none()
        {
            return Err(ModelError::InvalidTransition);
        }
        let key = reply_key(source);
        if let Some(notice) = self.reply_routes.get(&key).copied().flatten() {
            if self
                .request(notice)
                .is_some_and(|r| r.phase == RequestPhase::Queued)
            {
                self.recover_idle_request(notice, now_ms)?;
            }
        }
        self.reply_routes.insert(key, None);
        Ok(())
    }

    /// Reconcile retained replies, including callbacks recovered after restart
    /// and replies archived before routing existed. Save this change atomically
    /// with the queue before ordinary terminal submission can start.
    pub(crate) fn route_worker_replies(
        &mut self,
        now_ms: u64,
        waited: &BTreeSet<PromptId>,
    ) -> Result<bool, ModelError> {
        let mut groups: BTreeMap<RequestId, Vec<Request>> = BTreeMap::new();
        for request in self.requests().filter(|r| self.reply_needs_ack(r)) {
            let key = reply_key(request);
            if !self.reply_routes.contains_key(&key) {
                groups.entry(key).or_default().push(request.clone());
            }
        }
        let mut changed = false;
        for (key, group) in groups {
            if group.iter().any(|r| {
                waited.contains(&r.prompt.id)
                    || now_ms
                        < r.completed_at_ms
                            .unwrap_or(now_ms)
                            .saturating_add(REPLY_GRACE_MS)
            }) {
                continue;
            }
            let source = &group[0];
            let Some(orchestrator) = self.reply_orchestrator(source) else {
                continue;
            };
            let messages = group
                .iter()
                .map(|r| r.prompt.id.0)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let worker = self
                .agent(source.agent_id)
                .ok_or(ModelError::UnknownAgent(source.agent_id))?;
            let reply = source
                .pending_final
                .as_ref()
                .ok_or(ModelError::InvalidTransition)?;
            let body = if reply.text.chars().count() > INLINE_REPLY_CHARS {
                format!(
                    "{}…\nFull reply: bus message status {}",
                    reply.text.chars().take(200).collect::<String>(),
                    source.prompt.id.0
                )
            } else {
                reply.text.clone()
            };
            let text = format!(
                "{} (agent {}). Reply to message {messages} (request {}).\n{body}",
                worker.name, worker.id.0, source.id.0
            );
            let notice = self.submit_message_from(
                source.room_id,
                Draft {
                    text,
                    files: Vec::new(),
                    recipient_ids: [orchestrator].into(),
                },
                Author::Agent(source.agent_id),
                now_ms,
            )?[0];
            self.reply_routes.insert(key, Some(notice));
            changed = true;
        }
        Ok(changed)
    }
}

fn reply_key(request: &Request) -> RequestId {
    request.group.unwrap_or(request.id)
}
