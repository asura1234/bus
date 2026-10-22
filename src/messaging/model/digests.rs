//! Action acknowledgments are independent of transport delivery and request history.
use super::{
    reply_routing::{reply_key, ReplyRoute},
    worker_events::{WorkerEvent, WorkerEventKind},
    AgentId, AgentRecipients, Author, BusState, Draft, ModelError, RequestId, RequestPhase, RoomId,
    RoomKind, RuntimeStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(super) struct RoomDigest {
    /// Highest fact included in a durably queued digest, never an action ack.
    watermark: u64,
    armed: bool,
    last_digest_request: Option<RequestId>,
    last_digest_at_ms: Option<u64>,
    last_acknowledged_at_ms: Option<u64>,
    /// Source keys deduplicate replay; snapshots survive deletion of their agent/history.
    facts: BTreeMap<String, RoomFact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RoomFact {
    id: u64,
    room: RoomId,
    agent: Option<AgentId>,
    agent_name: String,
    at_ms: u64,
    text: String,
    next_action: String,
    acknowledged_at_ms: Option<u64>,
    agent_deleted: bool,
}

impl RoomDigest {
    fn refresh_armed(&mut self) {
        self.armed = self
            .facts
            .values()
            .any(|f| f.id > self.watermark && f.acknowledged_at_ms.is_none());
    }
}

impl BusState {
    fn record_room_fact(
        &mut self,
        room: RoomId,
        agent: Option<AgentId>,
        key: String,
        text: &str,
        next_action: &str,
        at_ms: u64,
    ) {
        let Some(orchestrator) = self.notice_orchestrator(room, None) else {
            return;
        };
        if self
            .room(room)
            .is_none_or(|r| r.kind != RoomKind::Work || r.deletion_pending)
            || self
                .room_digests
                .get(&orchestrator)
                .is_some_and(|r| r.facts.contains_key(&key))
        {
            return;
        }
        let name = agent
            .and_then(|id| self.agent(id))
            .map_or("bus", |a| a.name.as_str())
            .to_owned();
        let id = self.allocate_id();
        let record = self.room_digests.entry(orchestrator).or_default();
        record.armed = true;
        record.facts.insert(
            key,
            RoomFact {
                id,
                room,
                agent,
                agent_name: name,
                at_ms,
                text: text.to_owned(),
                next_action: next_action.to_owned(),
                acknowledged_at_ms: None,
                agent_deleted: false,
            },
        );
    }

    pub(super) fn record_digest_final(
        &mut self,
        worker: AgentId,
        room: RoomId,
        source: String,
        text: &str,
        at: u64,
    ) {
        let Some(tag) = status_tag(text) else {
            return;
        };
        let next = match tag {
            "READY FOR VERIFY" | "REVIEW READY" => "assign verification or record a hold",
            "BLOCKED" => "resolve the blocker or assign recovery",
            _ => "close or reuse the finished worker",
        };
        self.record_worker_room_fact(room, worker, source, text, next, at);
    }

    fn record_worker_room_fact(
        &mut self,
        room: RoomId,
        worker: AgentId,
        key: String,
        text: &str,
        next: &str,
        at: u64,
    ) {
        let Some(agent) = self.agent(worker) else {
            return;
        };
        let (home, orchestrates) = (agent.room_id, agent.orchestrates);
        // Do not turn the orchestrator's own digest answer into another wake.
        if orchestrates != Some(room) {
            self.record_room_fact(room, Some(worker), key.clone(), text, next, at);
        }
        if home != room && orchestrates.is_none() {
            self.record_room_fact(home, Some(worker), key, text, next, at);
        }
    }

    pub(super) fn record_digest_request(&mut self, id: RequestId, now: u64) {
        let Some(request) = self.request(id) else {
            return;
        };
        let group = request.group.unwrap_or(id);
        let (source, outcome) = if request.phase == RequestPhase::Abandoned {
            (
                format!("ended:{}", group.0),
                format!(
                    "abandoned: {}",
                    request.failure_reason.as_deref().unwrap_or("no reply")
                ),
            )
        } else if let Some(reason) = self.stall_reason(request, now) {
            let episode = self.agent(request.agent_id).map_or(0, |a| a.busy_revision);
            (
                format!("stalled:{}:{episode}", group.0),
                format!("stalled: {reason}"),
            )
        } else {
            return;
        };
        let (room, worker) = (request.room_id, request.agent_id);
        let text = format!(
            "Request {} {outcome}; message {}.\n{}",
            id.0,
            request.prompt.id.0,
            request.prompt.text.chars().take(200).collect::<String>()
        );
        self.record_worker_room_fact(
            room,
            worker,
            format!("request:{source}"),
            &text,
            "inspect the request and assign recovery",
            now,
        );
    }

    pub(super) fn record_digest_event(&mut self, key: &str, event: &WorkerEvent) {
        if matches!(
            event.kind,
            WorkerEventKind::Outcome | WorkerEventKind::Exited | WorkerEventKind::Idle
        ) {
            let source = if event.kind == WorkerEventKind::Outcome {
                format!("request:{key}")
            } else {
                format!("event:{key}")
            };
            self.record_worker_room_fact(
                event.room,
                event.agent,
                source,
                &event.text,
                "inspect the request and assign recovery",
                event.at_ms,
            );
        }
    }

    /// Availability without an active or queued task needs a disposition,
    /// including a fresh worker whose launch just became ready.
    pub(super) fn record_digest_idle(&mut self, worker: AgentId, now: u64) {
        let Some(agent) = self.agent(worker).filter(|a| {
            a.orchestrates.is_none()
                && !a.deletion_pending
                && a.shown_status() == RuntimeStatus::Idle
                && !a.dialog
                && a.current_request.is_none()
                && self.queued_requests(a.id).is_empty()
        }) else {
            return;
        };
        self.record_room_fact(
            agent.room_id,
            Some(worker),
            format!("idle:{}:{}", worker.0, agent.busy_revision),
            "Idle with no next task",
            "assign the next task or close the worker",
            now,
        );
    }

    pub(super) fn record_digest_notice(
        &mut self,
        room: RoomId,
        about: Option<AgentId>,
        id: super::PromptId,
        text: &str,
        at: u64,
    ) {
        self.record_room_fact(
            room,
            about,
            format!("notice:{}", id.0),
            text,
            "handle the notice or explicitly dismiss this fact",
            at,
        );
    }

    /// Reconcile retained pre-upgrade sources at startup, not on a timer.
    /// Stable source keys keep both acknowledgments and delivery watermarks intact.
    pub(crate) fn recover_room_digest_facts(&mut self) {
        let recovered_since = self.next_id;
        let replies: Vec<_> = self
            .requests()
            .filter(|r| r.phase == RequestPhase::Completed)
            .filter_map(|r| {
                r.pending_final
                    .as_ref()
                    .map(|f| (r.agent_id, r.room_id, r.group.unwrap_or(r.id), f.clone()))
            })
            .collect();
        for (agent, room, group, final_reply) in replies {
            self.record_digest_final(
                agent,
                room,
                format!("final:{}", group.0),
                &final_reply.text,
                final_reply.received_at_ms,
            );
        }
        let abandoned: Vec<_> = self
            .requests()
            .filter(|r| r.phase == RequestPhase::Abandoned)
            .map(|r| (r.id, r.completed_at_ms.unwrap_or(r.prompt.submitted_at_ms)))
            .collect();
        for (id, at) in abandoned {
            self.record_digest_request(id, at);
        }
        let events: Vec<_> = self
            .worker_events
            .iter()
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        for (key, event) in events {
            self.record_digest_event(&key, &event);
            if event.kind == WorkerEventKind::Final && !key.starts_with("turn-final:") {
                if let Some((_, body)) = event.text.split_once('\n') {
                    self.record_digest_final(
                        event.agent,
                        event.room,
                        format!("event:{key}"),
                        body,
                        event.at_ms,
                    );
                }
            }
        }
        let notices: Vec<_> = self
            .requests()
            .filter(|r| {
                r.prompt.author == Author::Bus
                    && !r.prompt.text.starts_with("Room digest:")
                    && self
                        .orchestrator_of(r.room_id)
                        .is_some_and(|a| a.id == r.agent_id)
            })
            .map(|r| (r.room_id, r.prompt.clone()))
            .collect();
        for (room, prompt) in notices {
            self.record_digest_notice(room, None, prompt.id, &prompt.text, prompt.submitted_at_ms);
        }
        let attention: Vec<_> = self
            .rooms()
            .flat_map(|r| self.notices(r.id))
            .filter_map(|p| p.attention_origin.as_ref().map(|o| (o.clone(), p.clone())))
            .collect();
        for (origin, prompt) in attention {
            if self
                .agent(origin.agent_id)
                .is_some_and(|a| a.orchestrates.is_none())
            {
                self.record_digest_notice(
                    origin.room_id,
                    Some(origin.agent_id),
                    prompt.id,
                    &prompt.text,
                    prompt.submitted_at_ms,
                );
            }
        }
        let agents: Vec<_> = self.agents().map(|a| (a.id, a.observed_at_ms)).collect();
        for (agent, at) in agents {
            self.record_digest_idle(agent, at);
        }
        self.ack_recovered_digest_actions(recovered_since);
    }

    fn ack_recovered_digest_actions(&mut self, recovered_since: u64) {
        let actions: Vec<_> = self
            .requests()
            .filter_map(|r| match r.prompt.author {
                Author::Agent(author) => Some((author, r.agent_id, r.prompt.submitted_at_ms)),
                _ => None,
            })
            .collect();
        for (orchestrator, record) in &mut self.room_digests {
            for fact in record
                .facts
                .values_mut()
                .filter(|f| f.id >= recovered_since)
            {
                let action = actions
                    .iter()
                    .filter(|(author, agent, at)| {
                        author == orchestrator && Some(*agent) == fact.agent && *at > fact.at_ms
                    })
                    .map(|(_, _, at)| *at)
                    .min();
                if let Some(at) = action {
                    fact.acknowledged_at_ms = Some(at);
                    record.last_acknowledged_at_ms =
                        Some(record.last_acknowledged_at_ms.unwrap_or(0).max(at));
                }
            }
            record.refresh_armed();
        }
    }

    pub(crate) fn room_digest_armed(&self, orchestrator: AgentId) -> bool {
        self.room_digests
            .get(&orchestrator)
            .is_some_and(|r| r.armed)
    }

    fn unrouted_worker_notice(&self, orchestrator: AgentId) -> bool {
        self.requests().any(|r| {
            self.reply_orchestrator(r) == Some(orchestrator)
                && self.reply_needs_ack(r)
                && matches!(
                    self.reply_routes.get(&reply_key(r)),
                    None | Some(ReplyRoute::Pending)
                )
        }) || self.worker_events.values().any(|event| {
            event.route == ReplyRoute::Pending
                && self.worker_orchestrator(event.agent) == Some(orchestrator)
        })
    }

    /// Only the ready-delivery boundary calls this. The request, fact watermark
    /// and source snapshots are saved together before any native typing.
    pub(crate) fn queue_room_digest(
        &mut self,
        orchestrator: AgentId,
        now: u64,
    ) -> Result<bool, ModelError> {
        let Some(agent) = self.agent(orchestrator).filter(|a| {
            a.orchestrates.is_some()
                && a.status == RuntimeStatus::Idle
                && !a.dialog
                && !a.deletion_pending
                && a.current_request.is_none()
                && self.queued_requests(a.id).is_empty()
        }) else {
            return Ok(false);
        };
        let Some(record) = self.room_digests.get(&orchestrator) else {
            return Ok(false);
        };
        // Preserve the reply batching window: availability must not outrun
        // the actual result the owner is being asked to disposition.
        if !record.armed || self.unrouted_worker_notice(orchestrator) {
            return Ok(false);
        }
        let mut pending: Vec<_> = record
            .facts
            .values()
            .filter(|f| f.acknowledged_at_ms.is_none())
            .collect();
        let watermark = pending.iter().map(|f| f.id).max().unwrap_or(0);
        if watermark <= record.watermark {
            return Ok(false);
        }
        pending.sort_by_key(|f| (f.at_ms, f.id));
        let room = agent.orchestrates.ok_or(ModelError::InvalidTransition)?;
        let mut text = format!(
            "Room digest: {} unacknowledged facts in room {}.\n",
            pending.len(),
            room.0
        );
        for fact in pending {
            let who = fact.agent.map_or_else(
                || "bus".to_owned(),
                |id| {
                    format!(
                        "{} (agent {}){}",
                        fact.agent_name,
                        id.0,
                        if fact.agent_deleted { " [deleted]" } else { "" }
                    )
                },
            );
            let summary = fact.text.chars().take(600).collect::<String>();
            text.push_str(&format!(
                "\n- Fact {}: {who}; age {}s; next owner: {} (agent {}) — {}.\n{}\n",
                fact.id,
                now.saturating_sub(fact.at_ms) / 1000,
                agent.name,
                orchestrator.0,
                fact.next_action,
                summary
            ));
        }
        text.push_str("\nAct on each fact: message its worker, close the worker, or explicitly dismiss it with bus digest ack FACT_ID. Reading this digest or replying to it does not acknowledge facts. No repeated wake is scheduled without new facts.");
        let request = self.submit_message_with(
            room,
            Draft {
                text,
                files: vec![],
                recipient_ids: [orchestrator].into(),
            },
            Author::Bus,
            now,
            true,
        )?[0];
        if let Some(record) = self.room_digests.get_mut(&orchestrator) {
            record.watermark = watermark;
            record.armed = false;
            record.last_digest_request = Some(request);
            record.last_digest_at_ms = Some(now);
        }
        tracing::info!(
            event = "bus.digest.queued",
            agent_id = orchestrator.0,
            room_id = room.0,
            watermark,
            request_id = request.0,
            "Room digest queued for a clean turn"
        );
        Ok(true)
    }

    pub(super) fn is_room_digest(&self, id: RequestId) -> bool {
        self.room_digests
            .values()
            .any(|r| r.last_digest_request == Some(id))
    }

    /// Only an accepted agent-authored dispatch counts; developer sends and
    /// transport-seen acknowledgments deliberately do not enter this path.
    pub(super) fn ack_digest_action(
        &mut self,
        author: &Author,
        recipients: &AgentRecipients,
        now: u64,
    ) {
        let Author::Agent(orchestrator) = author else {
            return;
        };
        let Some(record) = self.room_digests.get_mut(orchestrator) else {
            return;
        };
        for fact in record
            .facts
            .values_mut()
            .filter(|f| f.agent.is_some_and(|id| recipients.contains(&id)))
        {
            fact.acknowledged_at_ms.get_or_insert(now);
            record.last_acknowledged_at_ms = Some(now);
        }
        record.refresh_armed();
    }

    pub(super) fn ack_digest_deletion(&mut self, agent: AgentId, now: u64) {
        for record in self.room_digests.values_mut() {
            for fact in record.facts.values_mut().filter(|f| f.agent == Some(agent)) {
                fact.agent_deleted = true;
                fact.acknowledged_at_ms.get_or_insert(now);
                record.last_acknowledged_at_ms = Some(now);
            }
            record.refresh_armed();
        }
    }

    pub(crate) fn ack_digest_fact(&mut self, id: u64, now: u64) -> Result<(), String> {
        for record in self.room_digests.values_mut() {
            if let Some(fact) = record.facts.values_mut().find(|f| f.id == id) {
                fact.acknowledged_at_ms.get_or_insert(now);
                record.last_acknowledged_at_ms = Some(now);
                record.refresh_armed();
                return Ok(());
            }
        }
        Err(format!("Unknown digest fact {id}"))
    }
}

fn status_tag(text: &str) -> Option<&'static str> {
    let first = text.lines().next()?.trim_start();
    [
        "READY FOR VERIFY",
        "REVIEW READY",
        "MERGED",
        "DONE",
        "BLOCKED",
    ]
    .into_iter()
    .find(|tag| {
        first
            .get(..tag.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(tag))
            && first[tag.len()..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || matches!(c, ':' | '-' | '—' | '['))
    })
}
