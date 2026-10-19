//! Durable worker facts that the room orchestrator must not have to poll for.
use super::{
    reply_routing::{reply_key, ReplyRoute},
    AgentId, Author, BusState, CallbackDisposition, CallbackEventKind, CallbackRejection, PromptId,
    ProviderCallback, Request, RequestId, RequestPhase, RoomId, RoomKind, RuntimeStatus,
    TURN_SETTLE_MS,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WorkerEventKind {
    Final,
    Outcome,
    Idle,
    Exited,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct WorkerEvent {
    pub(super) agent: AgentId,
    pub(super) room: RoomId,
    pub(super) group: Option<RequestId>,
    pub(super) message: PromptId,
    pub(super) at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) episode: Option<u64>,
    pub(super) kind: WorkerEventKind,
    pub(super) text: String,
    pub(super) route: ReplyRoute,
}

impl BusState {
    pub(super) fn worker_orchestrator(&self, worker: AgentId) -> Option<AgentId> {
        let agent = self.agent(worker)?;
        let room = self.room(agent.room_id)?;
        let orchestrator = self.orchestrator_of(room.id)?;
        (room.kind == RoomKind::Work
            && !room.deletion_pending
            && !agent.deletion_pending
            && !orchestrator.deletion_pending
            && worker != orchestrator.id)
            .then_some(orchestrator.id)
    }

    /// Called only after correlation. Reject duplicate/foreign callbacks and
    /// retain the full text before the coordinator can acknowledge its spool.
    pub(super) fn record_worker_final(
        &mut self,
        callback: &ProviderCallback,
        disposition: &CallbackDisposition,
    ) {
        if !matches!(
            disposition,
            CallbackDisposition::Rejected(
                CallbackRejection::NoActiveRequest
                    | CallbackRejection::UnrelatedTurn
                    | CallbackRejection::WrongTurn
                    | CallbackRejection::WrongPrompt
                    | CallbackRejection::UnboundFinal
            )
        ) {
            return;
        }
        let CallbackEventKind::Final { text } = &callback.kind else {
            return;
        };
        let Some(agent) = self.agent(callback.agent_id) else {
            return;
        };
        if text.trim().is_empty()
            || self.worker_orchestrator(agent.id).is_none()
            || agent.session_binding_invalidated
            || agent.runtime_identity.launch_id.as_deref() != Some(&callback.launch_id)
            || agent.runtime_identity.session_id != callback.provider_session_id
        {
            return;
        }
        let Some(turn) = super::callbacks::provider_turn_key(
            &callback.launch_id,
            callback.provider_session_id.as_deref(),
            callback.provider_turn_id.as_deref(),
        ) else {
            return;
        };
        let owned: Vec<_> = self
            .requests()
            .filter(|r| {
                r.agent_id == agent.id
                    && r.expected_launch_id.as_deref() == Some(&callback.launch_id)
                    && r.provider_session_id == callback.provider_session_id
                    && r.provider_turn_id == callback.provider_turn_id
            })
            .collect();
        let late = self.late_worker_request(callback);
        let key = format!("final:{turn}");
        let orphan = matches!(
            disposition,
            CallbackDisposition::Rejected(
                CallbackRejection::NoActiveRequest | CallbackRejection::UnrelatedTurn
            )
        );
        if self.worker_events.contains_key(&key)
            || late.is_none()
                && (!orphan || !owned.is_empty() || self.consumed_provider_turns.contains(&turn))
        {
            return;
        }
        let group = late.map(reply_key);
        let label = late.map_or_else(
            || "Turn final".to_owned(),
            |r| {
                format!(
                    "Late reply to message {} (request {})",
                    r.prompt.id.0, r.id.0
                )
            },
        );
        let (room, worker) = (agent.room_id, agent.id);
        let Ok(message) =
            self.post_to_developer(room, worker, text.clone(), vec![], callback.occurred_at_ms)
        else {
            return;
        };
        let summary = format!(
            "{label}; message {}.\n{}",
            message.0,
            preview(text, &format!("bus history --room {}", room.0))
        );
        self.worker_events.insert(
            key,
            WorkerEvent {
                agent: worker,
                room,
                group,
                message,
                at_ms: callback.occurred_at_ms,
                episode: None,
                kind: WorkerEventKind::Final,
                text: summary,
                route: ReplyRoute::Pending,
            },
        );
        self.consumed_provider_turns.insert(turn);
    }

    /// A final for a closed owned turn must not wait behind newer unbound work.
    pub(crate) fn late_worker_request(&self, callback: &ProviderCallback) -> Option<&Request> {
        self.requests().find(|r| {
            r.agent_id == callback.agent_id
                && r.phase == RequestPhase::Abandoned
                && self.reply_orchestrator(r).is_some()
                && r.expected_launch_id.as_deref() == Some(&callback.launch_id)
                && r.provider_session_id == callback.provider_session_id
                && r.provider_turn_id.is_some()
                && r.provider_turn_id == callback.provider_turn_id
        })
    }

    /// Reconcile retained outcomes before archive or delivery. First upgrade
    /// deliberately backfills retained outcomes rather than guessing them seen.
    pub(crate) fn retain_worker_outcomes(&mut self, now_ms: u64) -> bool {
        let orchestrator_groups: std::collections::BTreeSet<_> = self
            .requests()
            .filter(|r| self.reply_orchestrator(r).is_some())
            .map(reply_key)
            .collect();
        let requests: Vec<_> = self
            .requests()
            .filter(|r| {
                self.worker_orchestrator(r.agent_id).is_some()
                    && self
                        .agent(r.agent_id)
                        .is_some_and(|a| a.room_id == r.room_id)
            })
            .filter(|r| {
                if r.pending_final.is_some() {
                    let group = reply_key(r);
                    r.phase == RequestPhase::Completed
                        && matches!(r.prompt.author, Author::Agent(_))
                        && !orchestrator_groups.contains(&group)
                        && !self.reply_routes.contains_key(&group)
                        && !self
                            .worker_events
                            .contains_key(&format!("turn-final:{}", group.0))
                } else {
                    !r.settled()
                        || !self
                            .worker_events
                            .contains_key(&format!("ended:{}", reply_key(r).0))
                }
            })
            .cloned()
            .collect();
        let before = self.worker_events.len();
        for request in requests {
            if request.pending_final.is_some() {
                self.retain_peer_final(&request);
                continue;
            }
            let outcome = if request.phase == RequestPhase::Abandoned {
                Some((
                    "ended",
                    format!(
                        "abandoned: {}",
                        request.failure_reason.as_deref().unwrap_or("abandoned")
                    ),
                ))
            } else {
                self.stall_reason(&request, now_ms)
                    .map(|reason| ("stalled", format!("stalled: {reason}")))
            };
            if let Some((class, reason)) = outcome {
                let idle = request.turn_ended_at_ms.map_or("", |_| "; went idle");
                self.record_request_event(
                    &request,
                    WorkerEventKind::Outcome,
                    class,
                    &format!("No reply ({reason}{idle})"),
                    request.completed_at_ms.unwrap_or(now_ms),
                );
            }
            let Some(agent) = self.agent(request.agent_id) else {
                continue;
            };
            let idle_since = request.turn_ended_at_ms.unwrap_or(agent.status_since_ms);
            let idle = self.reply_orchestrator(&request).is_some()
                && !request.settled()
                && !self.settling(&request)
                && agent.shown_status() == RuntimeStatus::Idle
                && !agent.dialog
                && agent.status_revision > agent.busy_revision
                && (request.awaiting_background
                    || self.turn_ended(&request)
                    || (agent.status_since_ms >= request.prompt.submitted_at_ms
                        && agent.busy_revision > request.submission_status_revision))
                && now_ms >= idle_since.saturating_add(TURN_SETTLE_MS);
            if idle {
                self.record_request_event(
                    &request,
                    WorkerEventKind::Idle,
                    "idle",
                    if request.has_held_background_reply() {
                        "Idle with open request (background shells pending)"
                    } else if request.awaiting_background {
                        "Idle with open request (background work pending)"
                    } else {
                        "Idle with open request (reply pending)"
                    },
                    idle_since,
                );
            }
        }
        before != self.worker_events.len()
    }

    fn retain_peer_final(&mut self, request: &Request) {
        let Some(reply) = request.pending_final.as_ref() else {
            return;
        };
        let group = reply_key(request);
        if request.phase != RequestPhase::Completed
            || !matches!(request.prompt.author, Author::Agent(_))
            || self.reply_routes.contains_key(&group)
        {
            return;
        }
        let text = format!(
            "Turn final.\n{}",
            preview(
                &reply.text,
                &format!("bus message status {}", request.prompt.id.0)
            )
        );
        self.record_request_event(
            request,
            WorkerEventKind::Final,
            "turn-final",
            &text,
            request.completed_at_ms.unwrap_or(reply.received_at_ms),
        );
    }

    /// Native positive evidence only; an unavailable list/socket is not exit.
    pub(crate) fn record_worker_exit(&mut self, worker: AgentId, now_ms: u64) {
        if self
            .agent(worker)
            .is_none_or(|a| !a.hook_setup_confirmed || a.startup.is_some())
        {
            return;
        }
        let requests: Vec<_> = self
            .requests()
            .filter(|r| {
                r.agent_id == worker && !r.settled() && self.reply_orchestrator(r).is_some()
            })
            .cloned()
            .collect();
        for request in requests {
            self.record_request_event(
                &request,
                WorkerEventKind::Exited,
                "exit",
                "Exited with open request (process_exited)",
                now_ms,
            );
        }
    }

    fn record_request_event(
        &mut self,
        request: &Request,
        kind: WorkerEventKind,
        class: &str,
        text: &str,
        at_ms: u64,
    ) {
        let group = reply_key(request);
        // A resumed background turn can go idle or stall again. Persist its
        // busy revision so stable polls/restarts dedup, while new work has its
        // own fact and cannot be hidden by an earlier printed status.
        let episode = (kind == WorkerEventKind::Idle || class == "stalled")
            .then(|| self.agent(request.agent_id).map_or(0, |a| a.busy_revision));
        let key = match episode {
            Some(episode) => format!("{class}:{}:{episode}", group.0),
            None => format!("{class}:{}", group.0),
        };
        self.worker_events
            .entry(key)
            .or_insert_with(|| WorkerEvent {
                agent: request.agent_id,
                room: request.room_id,
                group: Some(group),
                message: request.prompt.id,
                at_ms,
                episode,
                kind,
                text: format!(
                    "{}; message {} (request {}).\nInspect: bus message status {}",
                    preview(text, &format!("bus message status {}", request.prompt.id.0)),
                    request.prompt.id.0,
                    request.id.0,
                    request.prompt.id.0
                ),
                route: ReplyRoute::Pending,
            });
    }

    /// Build retention indexes once per archive/load, without copying bodies.
    pub(crate) fn worker_pending_sources(
        &self,
    ) -> (
        std::collections::BTreeSet<RequestId>,
        std::collections::BTreeSet<PromptId>,
    ) {
        let mut groups = std::collections::BTreeSet::new();
        let mut messages = std::collections::BTreeSet::new();
        for event in self
            .worker_events
            .values()
            .filter(|e| self.event_pending(e))
        {
            groups.extend(event.group);
            messages.insert(event.message);
        }
        (groups, messages)
    }

    pub(super) fn event_pending(&self, event: &WorkerEvent) -> bool {
        match event.route {
            ReplyRoute::Pending => true,
            ReplyRoute::Routed(id) => self.request(id).is_some_and(|r| !r.settled()),
            ReplyRoute::Seen => false,
        }
    }

    pub(super) fn prune_worker_events(&mut self, now_ms: u64) -> bool {
        let groups: std::collections::BTreeSet<_> = self.requests().map(reply_key).collect();
        let messages: std::collections::BTreeSet<_> = self
            .rooms()
            .flat_map(|room| self.notices(room.id).map(|p| p.id))
            .collect();
        let keep: std::collections::BTreeSet<_> = self
            .worker_events
            .iter()
            .filter(|(_, e)| {
                let superseded = e.episode.is_some_and(|episode| {
                    self.agent(e.agent)
                        .is_some_and(|a| a.busy_revision != episode)
                        && now_ms.saturating_sub(e.at_ms) > super::history::HISTORY_RETENTION_MS
                });
                self.worker_orchestrator(e.agent).is_some()
                    && (self.event_pending(e)
                        || !superseded
                            && (e.group.is_some_and(|group| groups.contains(&group))
                                || messages.contains(&e.message)))
            })
            .map(|(key, _)| key.clone())
            .collect();
        let before = self.worker_events.len();
        self.worker_events.retain(|key, _| keep.contains(key));
        before != self.worker_events.len()
    }
}

pub(super) fn preview(text: &str, pointer: &str) -> String {
    if text.chars().count() <= 600 {
        text.to_owned()
    } else {
        format!(
            "{}…\nFull text: {pointer}",
            text.chars().take(200).collect::<String>()
        )
    }
}
