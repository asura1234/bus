//! Durable actionable room facts; transport delivery never acknowledges action.
use super::{
    reply_routing::{reply_key, ReplyRoute},
    worker_events::{WorkerEvent, WorkerEventKind},
    AgentId, AgentRecipients, Author, BusState, Draft, ModelError, PromptId, RequestId,
    RequestPhase, RoomId, RoomKind, RuntimeStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod actions;
mod delivery;
mod recovery;

const FACT_TEXT_LIMIT: usize = 600;
const TOMBSTONE_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(super) struct RoomDigest {
    /// Highest fact actually delivered, independent of action acknowledgments.
    watermark: u64,
    armed: bool,
    last_digest_request: Option<RequestId>,
    pending_watermark: Option<u64>,
    previous_watermark: u64,
    last_digest_at_ms: Option<u64>,
    last_acknowledged_at_ms: Option<u64>,
    /// Recovery floor for expired ack tombstones; never suppresses live capture.
    pruned_through: u64,
    facts: BTreeMap<String, RoomFact>,
    tombstones: BTreeMap<String, FactTombstone>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RoomFact {
    id: u64,
    #[serde(default)]
    source_id: u64,
    room: RoomId,
    agent: Option<AgentId>,
    agent_name: String,
    at_ms: u64,
    text: String,
    next_action: String,
    // Read old ledgers; acknowledgment moves new records into tombstones.
    acknowledged_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FactTombstone {
    id: u64,
    agent: Option<AgentId>,
    acknowledged_at_ms: u64,
}

#[derive(Clone)]
pub(super) struct DigestSource {
    key: String,
    id: u64,
}

impl DigestSource {
    pub(super) fn new(key: String, id: u64) -> Self {
        Self { key, id }
    }
}

impl RoomDigest {
    fn refresh_armed(&mut self) {
        self.armed = self.facts.values().any(|f| f.id > self.watermark);
    }

    fn acknowledge(&mut self, key: &str, now: u64) {
        if let Some(fact) = self.facts.remove(key) {
            self.tombstones.insert(
                key.into(),
                FactTombstone {
                    id: fact.id,
                    agent: fact.agent,
                    acknowledged_at_ms: now,
                },
            );
            self.last_acknowledged_at_ms = Some(now);
        }
        self.refresh_armed();
    }
}

impl BusState {
    pub(super) fn has_unacknowledged_queue_pressure(&self, agent: AgentId, episode: u64) -> bool {
        let key = format!("queue:{}:{episode}", agent.0);
        self.agent(agent)
            .and_then(|worker| {
                self.notice_orchestrator(worker.orchestrates.unwrap_or(worker.room_id), None)
            })
            .and_then(|owner| self.room_digests.get(&owner))
            .and_then(|record| record.facts.get(&key))
            .is_some_and(|fact| fact.acknowledged_at_ms.is_none())
    }

    pub(super) fn record_queue_pressure_fact(
        &mut self,
        room: RoomId,
        agent: AgentId,
        source: DigestSource,
        text: &str,
        at: u64,
    ) {
        if let Some(fact) = self
            .notice_orchestrator(room, None)
            .and_then(|owner| self.room_digests.get_mut(&owner))
            .and_then(|record| record.facts.get_mut(&source.key))
        {
            // Keep the fact identity, delivery watermark and armed state;
            // draining or aging one backlog must not schedule another wake.
            let updated = text.chars().take(FACT_TEXT_LIMIT).collect::<String>();
            if fact.text != updated {
                fact.text = updated;
            }
            return;
        }
        // record_room_fact also honors acknowledgments of this episode.
        self.record_room_fact(
            room,
            Some(agent),
            source,
            text,
            "redistribute queued work or resolve the bottleneck",
            at,
        );
    }

    pub(super) fn record_room_fact(
        &mut self,
        room: RoomId,
        agent: Option<AgentId>,
        source: DigestSource,
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
            || self.room_digests.get(&orchestrator).is_some_and(|r| {
                r.facts.contains_key(&source.key) || r.tombstones.contains_key(&source.key)
            })
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
            source.key,
            RoomFact {
                id,
                source_id: source.id,
                room,
                agent,
                agent_name: name,
                at_ms,
                text: text.chars().take(FACT_TEXT_LIMIT).collect(),
                next_action: next_action.into(),
                acknowledged_at_ms: None,
            },
        );
    }

    pub(super) fn record_digest_final(
        &mut self,
        worker: AgentId,
        room: RoomId,
        source: DigestSource,
        text: &str,
        at: u64,
    ) {
        let tag = status_tag(text);
        if let Some(agent) = self.agents.get_mut(&worker) {
            agent.digest_tagged_idle_revision = tag.map(|_| agent.busy_revision);
        }
        let Some(tag) = tag else {
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
        source: DigestSource,
        text: &str,
        next: &str,
        at: u64,
    ) {
        let Some(agent) = self.agent(worker) else {
            return;
        };
        let (home, orchestrates) = (agent.room_id, agent.orchestrates);
        // An orchestrator's own digest answer cannot create another wake.
        if orchestrates != Some(room) {
            self.record_room_fact(room, Some(worker), source.clone(), text, next, at);
        }
        if home != room && orchestrates.is_none() {
            self.record_room_fact(home, Some(worker), source, text, next, at);
        }
    }

    pub(super) fn record_digest_request(&mut self, id: RequestId, now: u64) {
        if self.is_room_digest(id) {
            self.restore_abandoned_digest(id);
            return;
        }
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
        let blocked = request
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("provider_error:"));
        let text = format!(
            "{}Request {} {outcome}; message {}.\n{}",
            if blocked { "BLOCKED " } else { "" },
            id.0,
            request.prompt.id.0,
            request.prompt.text.chars().take(200).collect::<String>()
        );
        self.record_worker_room_fact(
            room,
            worker,
            DigestSource::new(format!("request:{source}"), group.0),
            &text,
            "inspect the request and assign recovery",
            now,
        );
    }

    pub(super) fn record_digest_event(&mut self, key: &str, event: &WorkerEvent) {
        if matches!(
            event.kind,
            WorkerEventKind::Outcome | WorkerEventKind::Exited
        ) {
            let source = if event.kind == WorkerEventKind::Outcome {
                format!("request:{key}")
            } else {
                format!("event:{key}")
            };
            self.record_worker_room_fact(
                event.room,
                event.agent,
                DigestSource::new(source, event.group.map_or(event.message.0, |r| r.0)),
                &event.text,
                "inspect the request and assign recovery",
                event.at_ms,
            );
        }
    }

    pub(super) fn record_digest_idle(&mut self, worker: AgentId, now: u64) {
        let Some(agent) = self.agent(worker).filter(|a| {
            a.orchestrates.is_none()
                && !a.deletion_pending
                && a.shown_status() == RuntimeStatus::Idle
                && !a.dialog
                && a.current_request.is_none()
                && self.queued_requests(a.id).is_empty()
                && a.digest_tagged_idle_revision != Some(a.busy_revision)
        }) else {
            return;
        };
        self.record_room_fact(
            agent.room_id,
            Some(worker),
            DigestSource::new(
                format!("idle:{}:{}", worker.0, agent.busy_revision),
                worker.0,
            ),
            "Idle with no next task",
            "assign the next task or close the worker",
            now,
        );
    }

    pub(super) fn record_digest_notice(
        &mut self,
        room: RoomId,
        about: Option<AgentId>,
        id: PromptId,
        text: &str,
        at: u64,
    ) {
        let Some(agent) = about else {
            return;
        };
        self.record_room_fact(
            room,
            Some(agent),
            DigestSource::new(format!("notice:{}:{}", id.0, agent.0), id.0),
            text,
            "handle the notice or explicitly dismiss this fact",
            at,
        );
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
                .is_none_or(|c| !c.is_alphanumeric())
    })
}
