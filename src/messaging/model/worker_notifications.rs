//! One batching/queue owner for replies and other worker events.
use super::{
    reply_routing::{reply_key, ReplyRoute},
    worker_events::{preview, WorkerEventKind},
    AgentId, Author, BusState, Draft, ModelError, PromptId, Request, RequestId, RequestPhase,
    RoomId,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
enum Key {
    Reply(RequestId),
    Event(String),
}
struct Part {
    key: Key,
    worker: AgentId,
    room: RoomId,
    at: u64,
    text: String,
    waited: bool,
}

impl BusState {
    pub(crate) fn route_worker_replies(
        &mut self,
        now_ms: u64,
        waited: &BTreeSet<PromptId>,
    ) -> Result<bool, ModelError> {
        let mut changed = self.retain_worker_outcomes(now_ms);
        let mut batches: BTreeMap<AgentId, Vec<Part>> = BTreeMap::new();
        for part in self
            .notification_parts(None, waited)
            .into_iter()
            .filter(|p| !p.waited)
        {
            batches.entry(part.worker).or_default().push(part);
        }
        for (worker, parts) in batches {
            if parts.iter().all(|p| now_ms < p.at.saturating_add(2_000)) {
                continue;
            }
            let Some(orchestrator) = self.worker_orchestrator(worker) else {
                continue;
            };
            let room = parts[0].room;
            let queued = self
                .requests()
                .find(|r| {
                    r.phase == RequestPhase::Queued
                        && r.prompt.author == Author::Agent(worker)
                        && r.agent_id == orchestrator
                        && self.is_reply_notification(r.id)
                        && now_ms <= r.prompt.submitted_at_ms.saturating_add(2_000)
                })
                .map(|r| r.id);
            let notice = if let Some(id) = queued {
                id
            } else {
                let text = Self::notification_text(&parts);
                self.submit_message_from(
                    room,
                    Draft {
                        text,
                        files: vec![],
                        recipient_ids: [orchestrator].into(),
                    },
                    Author::Agent(worker),
                    now_ms,
                )?[0]
            };
            for part in parts {
                match part.key {
                    Key::Reply(key) => {
                        self.reply_routes.insert(key, ReplyRoute::Routed(notice));
                    }
                    Key::Event(key) => {
                        if let Some(event) = self.worker_events.get_mut(&key) {
                            event.route = ReplyRoute::Routed(notice);
                        }
                    }
                }
            }
            self.refresh_worker_notification(notice, now_ms)?;
            changed = true;
        }
        Ok(changed)
    }

    fn notification_parts(
        &self,
        notice: Option<RequestId>,
        waited: &BTreeSet<PromptId>,
    ) -> Vec<Part> {
        let mut parts = Vec::new();
        let mut replies: BTreeMap<RequestId, Vec<&Request>> = BTreeMap::new();
        for request in self.requests().filter(|r| self.reply_needs_ack(r)) {
            let key = reply_key(request);
            if selected(
                self.reply_routes
                    .get(&key)
                    .copied()
                    .unwrap_or(ReplyRoute::Pending),
                notice,
            ) {
                replies.entry(key).or_default().push(request);
            }
        }
        for (key, group) in replies {
            let source = group[0];
            let Some(reply) = source.reply() else {
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
            parts.push(Part {
                key: Key::Reply(key),
                worker: source.agent_id,
                room: source.room_id,
                at: source.completed_at_ms.unwrap_or(reply.received_at_ms),
                text: format!(
                    "Reply to message {messages} (request {}).\n{}",
                    source.id.0,
                    preview(
                        &reply.text,
                        &format!("bus message status {}", source.prompt.id.0)
                    )
                ),
                waited: group.iter().any(|r| waited.contains(&r.prompt.id)),
            });
        }
        for (key, event) in &self.worker_events {
            if !selected(event.route, notice) {
                continue;
            }
            let author_waits = matches!(event.kind, WorkerEventKind::Outcome)
                && self.requests().any(|r| {
                    event.group == Some(reply_key(r))
                        && self.reply_orchestrator(r).is_some()
                        && waited.contains(&r.prompt.id)
                });
            parts.push(Part {
                key: Key::Event(key.clone()),
                worker: event.agent,
                room: event.room,
                at: event.at_ms,
                text: event.text.clone(),
                waited: author_waits,
            });
        }
        parts.sort_by_key(|p| p.at);
        parts
    }

    fn notification_text(parts: &[Part]) -> String {
        let mut text = String::new();
        let room = parts.first().map_or(RoomId(0), |p| p.room);
        for (index, part) in parts.iter().enumerate() {
            if text.chars().count() + part.text.chars().count() > 3_500 {
                text.push_str(&format!(
                    "\n{} more events. Full details: bus history --room {}",
                    parts.len() - index,
                    room.0
                ));
                break;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&part.text);
        }
        text
    }

    /// A post-flush acknowledgement can remove one component without erasing
    /// another event sharing its queued message. Submitted payloads are immutable.
    pub(super) fn refresh_worker_notification(
        &mut self,
        notice: RequestId,
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let Some(request) = self
            .request(notice)
            .filter(|r| r.phase == RequestPhase::Queued)
        else {
            return Ok(());
        };
        if !matches!(request.prompt.author, Author::Agent(_)) {
            return Ok(());
        }
        let parts = self.notification_parts(Some(notice), &BTreeSet::new());
        if parts.is_empty() {
            self.recover_idle_request(notice, now_ms)?;
        } else {
            let text = Self::notification_text(&parts);
            if let Some(request) = self.requests.get_mut(&notice) {
                request.prompt.text = text;
            }
        }
        Ok(())
    }

    pub(crate) fn worker_events_for(&self, request: &Request) -> Vec<(String, String)> {
        self.worker_events
            .iter()
            .filter(|(_, event)| event.group == Some(reply_key(request)))
            .map(|(key, event)| (key.clone(), event.text.clone()))
            .collect()
    }

    pub(crate) fn worker_outcomes_for(&self, request: &Request) -> Vec<(String, String)> {
        if self.reply_orchestrator(request).is_none() {
            return vec![];
        }
        self.worker_events
            .iter()
            .filter(|(_, event)| {
                event.group == Some(reply_key(request))
                    && event.route != ReplyRoute::Seen
                    && matches!(event.kind, WorkerEventKind::Outcome | WorkerEventKind::Idle)
            })
            .map(|(key, event)| (key.clone(), event.text.clone()))
            .collect()
    }

    pub(crate) fn see_worker_outcomes(
        &mut self,
        request: RequestId,
        author: AgentId,
        keys: &[String],
        now_ms: u64,
    ) -> Result<(), ModelError> {
        let source = self
            .request(request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if self.reply_orchestrator(source) != Some(author) || keys.is_empty() {
            return Err(ModelError::InvalidTransition);
        }
        let group = reply_key(source);
        for key in keys {
            if self.worker_events.get(key).is_none_or(|event| {
                event.group != Some(group)
                    || !matches!(event.kind, WorkerEventKind::Outcome | WorkerEventKind::Idle)
            }) {
                return Err(ModelError::InvalidTransition);
            }
        }
        let mut notices = BTreeSet::new();
        for key in keys {
            if let Some(event) = self.worker_events.get_mut(key) {
                notices.extend(event.route.notification());
                event.route = ReplyRoute::Seen;
            }
        }
        for notice in notices {
            self.refresh_worker_notification(notice, now_ms)?;
        }
        Ok(())
    }
}

fn selected(route: ReplyRoute, notice: Option<RequestId>) -> bool {
    match notice {
        Some(id) => route == ReplyRoute::Routed(id),
        None => route == ReplyRoute::Pending,
    }
}
