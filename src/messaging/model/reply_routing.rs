//! Durable routing of captured replies back to their requesters.
use super::{AgentId, Author, BusState, ModelError, Request, RequestId};
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
            .map(|request| request.id)
            .filter(|key| !self.reply_routes.contains_key(key))
            .collect();
        let changed = !pending.is_empty();
        self.reply_routes
            .extend(pending.into_iter().map(|key| (key, ReplyRoute::Pending)));
        changed
    }

    /// Reply wakes are ordinary messages to the original stable author ID.
    /// A wake's captured final must not bounce back automatically; an explicit
    /// recipient answer is still an intentional reply.
    pub(crate) fn reply_recipient(&self, request: &Request) -> Option<AgentId> {
        let Author::Agent(author) = request.prompt.author else {
            return None;
        };
        let author = self.agent(author)?;
        let worker = self.agent(request.agent_id)?;
        let room = self.room(request.room_id)?;
        let captured_wake = request.bus_generated
            && request
                .completed_reply
                .as_ref()
                .is_some_and(|reply| reply.group.is_some());
        (!captured_wake
            && author.id != worker.id
            && !author.deletion_pending
            && !worker.deletion_pending
            && !room.deletion_pending)
            .then_some(author.id)
    }

    /// Old routes belonged only to the room orchestrator, but used a group's
    /// lead ID even when that lead was authored by the developer or a peer.
    pub(crate) fn upgrade_requester_reply_routes(&mut self) {
        if self.reply_routes_by_request {
            return;
        }
        let routes = std::mem::take(&mut self.reply_routes);
        for (group, route) in routes {
            let keys: Vec<_> = self
                .requests()
                .filter(|request| {
                    reply_key(request) == group
                        && request.reply().is_some()
                        && self.reply_orchestrator(request).is_some()
                })
                .map(|request| request.id)
                .collect();
            for key in keys {
                self.reply_routes.insert(key, route);
            }
        }
        self.reply_routes_by_request = true;
    }

    pub(crate) fn reply_orchestrator(&self, request: &Request) -> Option<AgentId> {
        let worker = self.agent(request.agent_id)?;
        let room = self.room(request.room_id)?;
        let orchestrator = self.orchestrator_of(room.id)?;
        (!request.bus_generated
            && request.prompt.author == Author::Agent(orchestrator.id)
            && worker.room_id == room.id
            && worker.id != orchestrator.id
            && !worker.deletion_pending
            && !orchestrator.deletion_pending
            && !room.deletion_pending)
            .then_some(orchestrator.id)
    }

    pub(crate) fn reply_needs_ack(&self, request: &Request) -> bool {
        request
            .reply()
            .is_some_and(|reply| !reply.text.trim().is_empty())
            && self.reply_recipient(request).is_some()
            && self.reply_routes.get(&request.id) != Some(&ReplyRoute::Seen)
            && (self.routing_source(request) || self.reply_routes.contains_key(&request.id))
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
        match self.reply_routes.get(&request.id) {
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
        if self.reply_recipient(source) != Some(author) || source.reply().is_none() {
            return Err(ModelError::InvalidTransition);
        }
        let group = reply_key(source);
        let keys: Vec<_> = self
            .requests()
            .filter(|member| {
                reply_key(member) == group
                    && member.prompt.author == Author::Agent(author)
                    && member.reply().is_some()
            })
            .map(|member| member.id)
            .collect();
        let mut notices = BTreeSet::new();
        for key in keys {
            notices.extend(
                self.reply_routes
                    .get(&key)
                    .and_then(|route| route.notification()),
            );
            self.reply_routes.insert(key, ReplyRoute::Seen);
        }
        for notice in notices {
            self.refresh_worker_notification(notice, now_ms)?;
        }
        Ok(())
    }
}

pub(super) fn reply_key(request: &Request) -> RequestId {
    request.reply_key()
}
