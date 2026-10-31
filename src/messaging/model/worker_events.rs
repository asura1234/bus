//! Durable worker facts that the room orchestrator must not have to poll for.
use super::{
    reply_routing::{reply_key, ReplyRoute},
    AgentId, Author, BusState, CallbackDisposition, CallbackEventKind, CallbackRejection, PromptId,
    ProviderCallback, Request, RequestId, RequestPhase, RoomId, RoomKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WorkerEventKind {
    Final,
    Outcome,
    /// Retired "Idle with open request" notice; kept so saved state still loads.
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

/// Cancelling retired inputs preserves history without creating replacement work.
fn retired_automation(request: &Request) -> bool {
    request.prompt.author == Author::Bus
        && matches!(
            request.failure_reason.as_deref(),
            Some(
                "github_watch_retired"
                    | "room_digest_retired"
                    | "messaging_onboarding_retired"
                    | "operational_notice_retired"
            )
        )
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
                    | CallbackRejection::BeforeSubmissionBoundary
                    | CallbackRejection::DuplicateFinal
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
        let owned = self.requests().any(|r| {
            r.agent_id == agent.id
                && r.expected_launch_id.as_deref() == Some(&callback.launch_id)
                && r.provider_session_id == callback.provider_session_id
                && r.provider_turn_id == callback.provider_turn_id
        });
        let late = self.late_worker_request(callback);
        // A later request adopting this turn cannot change a final's identity.
        // Text keys deduplicate autonomous replays and preserve each changed
        // final of a closed turn, including across coordinator restarts.
        let key = format!(
            "final:{turn}:{}",
            crate::utils::time::digest(text.as_bytes())
        );
        let orphan = matches!(
            disposition,
            CallbackDisposition::Rejected(
                CallbackRejection::NoActiveRequest | CallbackRejection::UnrelatedTurn
            )
        );
        if self.worker_events.contains_key(&key)
            || late.is_none() && (!orphan || owned || self.consumed_provider_turns.contains(&turn))
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
        // This request's own copy suppresses its late final. An ungrouped
        // final still yields to any earlier copy, so an explicit send is not
        // posted again as Turn final.
        if self.final_text_already_posted(late, room, worker, text) {
            self.note_result_posted(turn, callback.occurred_at_ms);
            return;
        }
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
        self.note_result_posted(turn, callback.occurred_at_ms);
    }

    /// The body is already the result. Remember the turn and drop a quiet
    /// no-reply that this result replaces.
    fn note_result_posted(&mut self, turn: String, now_ms: u64) {
        self.consumed_provider_turns.insert(turn);
        let _changed = self
            .retract_suppressed_quiet_outcomes(now_ms)
            .unwrap_or(true);
    }

    /// A final for a closed owned turn must not wait behind newer unbound work.
    /// Completion can have delivered an interim (the background hold cap, or
    /// Codex Stop without task metadata). Keep the original outcome immutable
    /// and route changed text as a follow-up, under the same attested turn.
    pub(crate) fn late_worker_request(&self, callback: &ProviderCallback) -> Option<&Request> {
        self.requests().find(|r| {
            // Coalesced and steered members share the lead's submission and
            // final. The member proves who awaits the answer; the lead proves
            // which provider turn owns it, including its callback boundary.
            let Some(lead) = self.request(r.group.unwrap_or(r.id)) else {
                return false;
            };
            r.agent_id == callback.agent_id
                && lead.agent_id == r.agent_id
                && lead
                    .submission_boundary
                    .is_some_and(|b| callback.sequence > b)
                && r.completed_reply
                    .as_ref()
                    .is_none_or(|reply| reply.group.is_some())
                && (r.phase == RequestPhase::Abandoned
                    || r.phase == RequestPhase::Completed
                        && lead.trusted_start_bound
                        && matches!(&callback.kind, CallbackEventKind::Final { text }
                            if r.reply().is_some_and(|reply| reply.text != *text)))
                && self.reply_orchestrator(r).is_some()
                && lead.expected_launch_id.as_deref() == Some(&callback.launch_id)
                && lead.provider_session_id == callback.provider_session_id
                && lead.provider_turn_id.is_some()
                && lead.provider_turn_id == callback.provider_turn_id
        })
    }

    /// Reconcile retained outcomes before archive or delivery. First upgrade
    /// deliberately backfills retained outcomes rather than guessing them seen.
    pub(crate) fn retain_worker_outcomes(&mut self, now_ms: u64) -> bool {
        let requests: Vec<_> = self
            .requests()
            .filter(|r| !retired_automation(r))
            .filter(|r| self.event_backfill_source(r))
            .filter(|r| {
                self.worker_orchestrator(r.agent_id).is_some()
                    && self
                        .agent(r.agent_id)
                        .is_some_and(|a| a.room_id == r.room_id)
            })
            // A reply to a peer-started request belongs to that peer; copying it to the
            // orchestrator duplicated what the worker reports itself (bus-communication-guide.md).
            .filter(|r| r.reply().is_none())
            .filter(|r| {
                !r.settled()
                    || !self
                        .worker_events
                        .contains_key(&format!("ended:{}", reply_key(r).0))
            })
            .cloned()
            .collect();
        let before = self.worker_events.len();
        for request in requests {
            // Whoever interrupted or deleted the agent ended these requests on purpose;
            // a notice per queued request repeats that decision back to the sender.
            let expected_end = request.failure_reason.as_deref() == Some(super::INTERRUPTED)
                || self
                    .agent(request.agent_id)
                    .is_none_or(|a| a.deletion_pending);
            let outcome = self.outcome_to_post(&request, now_ms, expected_end);
            if let Some((class, reason)) = outcome {
                if class == "ended" {
                    // A terminal closer replaces an earlier stall sample. The
                    // replacement is the only outcome this request keeps.
                    let _withdrawn = self.withdraw_stall_outcomes(&request, now_ms).is_ok();
                }
                let idle = request.turn_ended_at_ms.map_or("", |_| "; went idle");
                self.record_request_event(
                    &request,
                    WorkerEventKind::Outcome,
                    class,
                    &format!("No reply ({reason}{idle})"),
                    request.completed_at_ms.unwrap_or(now_ms),
                );
            }
        }
        let retracted = self
            .retract_suppressed_quiet_outcomes(now_ms)
            .unwrap_or(true);
        retracted || before != self.worker_events.len()
    }

    /// A result or a restart that now covers this request replaces a quiet
    /// no-reply. Remove a quiet `ended` outcome that is still pending or queued
    /// and not yet delivered. Leave the late final and a real outcome in place,
    /// and do not record a replacement no-reply.
    fn retract_suppressed_quiet_outcomes(
        &mut self,
        now_ms: u64,
    ) -> Result<bool, super::ModelError> {
        let pending: Vec<(String, RequestId, Option<RequestId>)> = self
            .worker_events
            .iter()
            .filter(|(key, event)| {
                event.kind == WorkerEventKind::Outcome
                    && event.route != ReplyRoute::Seen
                    && key.starts_with("ended:")
                    && stored_quiet_ended(&event.text)
            })
            .filter_map(|(key, event)| {
                event.group.map(|group| {
                    let notice = match event.route {
                        ReplyRoute::Routed(id) => Some(id),
                        ReplyRoute::Pending | ReplyRoute::Seen => None,
                    };
                    (key.clone(), group, notice)
                })
            })
            .collect();
        let drop_keys: Vec<(String, Option<RequestId>)> = pending
            .into_iter()
            .filter(|(_, group, notice)| self.quiet_outcome_now_suppressed(*group, *notice, now_ms))
            .map(|(key, _, notice)| (key, notice))
            .collect();
        let changed = !drop_keys.is_empty();
        let mut notices = std::collections::BTreeSet::new();
        for (key, notice) in drop_keys {
            self.worker_events.remove(&key);
            notices.extend(notice);
        }
        self.refresh_withdrawn(notices, now_ms)?;
        Ok(changed)
    }

    /// A quiet ended outcome stays only while it is still the right notice.
    /// A restart, or this request's own result, withdraws a pending one and a
    /// copy that is queued but not yet delivered.
    fn quiet_outcome_now_suppressed(
        &self,
        group: RequestId,
        notice: Option<RequestId>,
        now_ms: u64,
    ) -> bool {
        if notice.is_some_and(|id| self.request(id).is_some_and(|queued| queued.settled())) {
            return false;
        }
        let Some(request) = self.request(group) else {
            return false;
        };
        if request.phase != RequestPhase::Abandoned {
            return false;
        }
        let closer = self.named_abandon_reason(request);
        self.suppress_outcome(request, now_ms, &closer)
    }

    /// Drop stall samples for `request` so a terminal closer is the only outcome.
    fn withdraw_stall_outcomes(
        &mut self,
        request: &Request,
        now_ms: u64,
    ) -> Result<(), super::ModelError> {
        let group = reply_key(request).0;
        let exact = format!("stalled:{group}");
        let prefix = format!("stalled:{group}:");
        let keys: Vec<String> = self
            .worker_events
            .keys()
            .filter(|key| *key == &exact || key.starts_with(&prefix))
            .cloned()
            .collect();
        let mut notices = std::collections::BTreeSet::new();
        for key in keys {
            if let Some(event) = self.worker_events.remove(&key) {
                if let ReplyRoute::Routed(id) = event.route {
                    if self.request(id).is_some_and(|notice| !notice.settled()) {
                        notices.insert(id);
                    }
                }
            }
        }
        self.refresh_withdrawn(notices, now_ms)
    }

    fn refresh_withdrawn(
        &mut self,
        notices: std::collections::BTreeSet<RequestId>,
        now_ms: u64,
    ) -> Result<(), super::ModelError> {
        for notice in notices {
            self.refresh_worker_notification(notice, now_ms)?;
        }
        Ok(())
    }

    /// One stall or one abandon for this request, never both, and never a
    /// second sample of a stall that was already posted. A terminal closer
    /// still posts when an earlier stall sample exists; a quiet abandon does not.
    fn outcome_to_post(
        &self,
        request: &Request,
        now_ms: u64,
        expected_end: bool,
    ) -> Option<(&'static str, String)> {
        if self.posted_class(request, "ended") {
            return None;
        }
        if request.phase == RequestPhase::Abandoned {
            if expected_end {
                return None;
            }
            let closer = self.named_abandon_reason(request);
            if self.suppress_outcome(request, now_ms, &closer) {
                return None;
            }
            if self.posted_class(request, "stalled") && quiet_reason(&closer) {
                return None;
            }
            Some(("ended", format!("abandoned: {closer}")))
        } else if self.posted_class(request, "stalled") {
            None
        } else {
            let reason = self.stall_reason(request, now_ms)?;
            if self.suppress_outcome(request, now_ms, &reason) {
                return None;
            }
            Some(("stalled", format!("stalled: {reason}")))
        }
    }

    /// `abandoned: abandoned` names nothing. Use the recorded closer, or say
    /// the request was superseded or recovered while idle.
    fn named_abandon_reason(&self, request: &Request) -> String {
        let raw = request.failure_reason.as_deref().unwrap_or("").trim();
        if !raw.is_empty() && raw != "abandoned" {
            return raw.to_owned();
        }
        let at = request
            .completed_at_ms
            .unwrap_or(request.prompt.submitted_at_ms);
        if self.requests().any(|other| {
            other.agent_id == request.agent_id
                && other.id != request.id
                && other.prompt.submitted_at_ms >= at
        }) {
            "superseded".to_owned()
        } else {
            "idle".to_owned()
        }
    }

    fn suppress_outcome(&self, request: &Request, now_ms: u64, reason: &str) -> bool {
        // A provider error, startup failure, or a submission still sitting
        // unsent stays. Same-minute silence is only for a no-reply that this
        // request's own result already replaced.
        if !quiet_reason(reason) {
            return false;
        }
        let at = request.completed_at_ms.unwrap_or(now_ms);
        if self.result_in_same_minute(request, at) {
            return true;
        }
        reason == "reply_not_captured"
            && (self.captured_reply_already_sent(request)
                || self.explicit_fallback_after_close(request)
                || self.restart_covers(request, now_ms))
    }

    fn posted_class(&self, request: &Request, class: &str) -> bool {
        let group = reply_key(request).0;
        let exact = format!("{class}:{group}");
        let prefix = format!("{class}:{group}:");
        self.worker_events
            .keys()
            .any(|key| key == &exact || key.starts_with(&prefix))
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
        // The saved screen follows the short notice whole, never previewed away.
        let snapshot = self
            .agent(worker)
            .and_then(|a| a.exit_snapshot.as_ref())
            .map(super::ExitSnapshot::notice);
        for request in requests {
            self.record_request_event_with(
                &request,
                WorkerEventKind::Exited,
                "exit",
                "Exited with open request (process_exited)",
                now_ms,
                snapshot.as_deref(),
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
        // One outcome per group: name its steered messages once per distinct
        // reason/receipt, rather than letting repeated explanations hide it
        // behind the notification payload limit.
        let mut groups = std::collections::BTreeMap::<_, Vec<&Request>>::new();
        for member in self
            .requests()
            .filter(|r| reply_key(r) == reply_key(request))
        {
            if member.steered && member.phase == RequestPhase::Abandoned && member.reply().is_none()
            {
                if let Some(error) = member
                    .failure_reason
                    .as_deref()
                    .filter(|r| r.starts_with("provider_error: "))
                {
                    groups
                        .entry((error, member.trusted_start_bound))
                        .or_default()
                        .push(member);
                }
            }
        }
        let steered = groups
            .values()
            .filter_map(|members| self.steered_failure_notice(members))
            .collect::<Vec<_>>()
            .join("\n");
        let detail = if steered.is_empty() {
            (request.failure_reason.as_deref() == Some(super::AGENT_STARTUP_FAILED))
                .then_some(request.submission_error.as_deref())
                .flatten()
        } else {
            Some(steered.as_str())
        };
        self.record_request_event_with(request, kind, class, text, at_ms, detail);
    }

    fn record_request_event_with(
        &mut self,
        request: &Request,
        kind: WorkerEventKind,
        class: &str,
        text: &str,
        at_ms: u64,
        detail: Option<&str>,
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
                    "{}; message {} (request {}).\nInspect: bus message status {}{}",
                    preview(text, &format!("bus message status {}", request.prompt.id.0)),
                    request.prompt.id.0,
                    request.id.0,
                    request.prompt.id.0,
                    detail
                        .map(|detail| format!("\n{detail}"))
                        .unwrap_or_default()
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
        let groups: std::collections::BTreeSet<_> = self
            .requests()
            .filter(|r| self.routing_source(r))
            .map(reply_key)
            .collect();
        let messages: std::collections::BTreeSet<_> = self
            .rooms()
            .flat_map(|room| {
                self.notices(room.id)
                    .filter(|p| p.submitted_at_ms >= self.routing_before_ms)
                    .map(|p| p.id)
            })
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

/// The stored notice names a quiet closer, not a provider or startup failure.
fn stored_quiet_ended(text: &str) -> bool {
    ["reply_not_captured", "idle", "superseded"]
        .into_iter()
        .any(|closer| {
            let needle = format!("abandoned: {closer}");
            text.match_indices(&needle).any(|(at, _)| {
                let tail = &text[at + needle.len()..];
                tail.is_empty() || matches!(tail.chars().next(), Some(';' | ')' | ' ' | '\n' | '.'))
            })
        })
}

fn quiet_reason(reason: &str) -> bool {
    matches!(reason, "reply_not_captured" | "idle" | "superseded")
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
