//! Durable routing of a worker's captured reply back to its room orchestrator.
use super::{AgentId, Author, BusState, ModelError, Request, RequestId, RequestPhase};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReplyRoute {
    Pending,
    Routed(RequestId),
    Seen,
}

impl ReplyRoute {
    pub(super) fn notification(self) -> Option<RequestId> {
        match self {
            Self::Routed(id) => Some(id),
            Self::Pending | Self::Seen => None,
        }
    }
}

impl BusState {
    /// Capture responsibility before retiring routing metadata. Permanent audit
    /// records below the recovery floor must not become new work on restart.
    pub(super) fn retain_unrouted_worker_replies(&mut self) -> bool {
        let pending: BTreeSet<_> = self
            .requests()
            .filter(|r| self.reply_needs_ack(r))
            .map(reply_key)
            .filter(|key| !self.reply_routes.contains_key(key))
            .collect();
        let changed = !pending.is_empty();
        self.reply_routes
            .extend(pending.into_iter().map(|key| (key, ReplyRoute::Pending)));
        changed
    }

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
            && self.reply_routes.get(&reply_key(request)) != Some(&ReplyRoute::Seen)
            && (self.routing_source(request) || self.reply_routes.contains_key(&reply_key(request)))
    }

    pub(crate) fn is_reply_notification(&self, request: RequestId) -> bool {
        self.reply_routes
            .values()
            .any(|route| route.notification() == Some(request))
            || self
                .worker_events
                .values()
                .any(|event| event.route.notification() == Some(request))
    }

    pub(crate) fn reply_delivery_pending(&self, request: &Request) -> bool {
        match self.reply_routes.get(&reply_key(request)) {
            Some(ReplyRoute::Pending) => self.reply_needs_ack(request),
            Some(ReplyRoute::Routed(id)) => {
                self.request(*id).is_some_and(|notice| !notice.settled())
            }
            Some(ReplyRoute::Seen) | None => false,
        }
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
        let notice = self
            .reply_routes
            .get(&key)
            .and_then(|route| route.notification());
        self.reply_routes.insert(key, ReplyRoute::Seen);
        if let Some(notice) = notice {
            self.refresh_worker_notification(notice, now_ms)?;
        }
        Ok(())
    }
}

pub(super) fn reply_key(request: &Request) -> RequestId {
    request.group.unwrap_or(request.id)
}
