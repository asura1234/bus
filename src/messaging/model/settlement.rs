//! Close finished turns after giving their completion callbacks time to arrive.
use super::{
    callbacks::provider_turn_key, AgentId, BusState, ModelError, PendingFinal, Request, RequestId,
    RequestPhase, RuntimeStatus, TURN_SETTLE_MS,
};

/// How the provider's own record says a turn ended, for a turn whose
/// completion callback never came.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RecordedTurnEnd {
    /// The provider error that ended the turn.
    Error(String),
    /// The provider proved completion but recorded no final answer.
    NoReply,
    /// The final answer of provider turn `turn_id`, attributed to the
    /// request, with every prompt the provider recorded for that turn.
    Final {
        turn_id: String,
        text: String,
        prompts: Vec<String>,
    },
}

/// Why an ended turn closed without a reply once its agent went unavailable.
const AGENT_UNAVAILABLE: &str = "agent_unavailable";

impl BusState {
    /// Reconcile unclaimed steering after this tick's hooks. Native Idle is
    /// an end boundary, never evidence that input shared the previous answer.
    pub(crate) fn settle_unclaimed_steering(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        mut recorded_end: impl FnMut(&Request) -> Option<RecordedTurnEnd>,
    ) -> bool {
        let Some(owner) = self.agents.get(&agent).filter(|a| {
            matches!(a.status, RuntimeStatus::Idle | RuntimeStatus::Unavailable)
                && !a.dialog
                && !a.deletion_pending
                && !a.session_binding_invalidated
        }) else {
            return false;
        };
        let Some(lead) = owner
            .current_request
            .and_then(|id| self.requests.get(&id))
            .filter(|r| r.phase == RequestPhase::Completed && r.pending_final.is_some())
            .cloned()
        else {
            return false;
        };
        let members = self.group_members(lead.id);
        if let Some(RecordedTurnEnd::Final {
            turn_id, prompts, ..
        }) = recorded_end(&lead)
        {
            if lead.provider_turn_id.as_deref() == Some(&turn_id) {
                for id in &members {
                    if self.requests.get(id).is_some_and(|member| {
                        member.steered
                            && !member.trusted_start_bound
                            && !member.settled()
                            && self.recorded_for(member, lead.id, &members, &prompts)
                    }) {
                        self.bind_recorded_member(*id, lead.provider_session_id.clone(), &turn_id);
                    }
                }
            }
        }
        let _ = self.complete_pending_final(agent, now_ms);
        let mut uncertain = None;
        for id in &members {
            let Some(member) = self.requests.get(id).filter(|m| !m.settled()).cloned() else {
                continue;
            };
            self.recover_steered_answer(&lead, &member, &members, recorded_end(&member), now_ms);
        }
        // A later member's transcript lookup can prove earlier inputs too.
        // Resolve all available evidence before declaring any member uncertain.
        for id in &members {
            if let Some(member) = self.requests.get_mut(id).filter(|m| !m.settled()) {
                member.group = None;
                member.phase = RequestPhase::Abandoned;
                member.completed_at_ms = Some(now_ms);
                member.failure_reason = Some("submission_unconfirmed".into());
                member.uncertain_outcome = true;
                uncertain.get_or_insert(member.prompt.id);
            }
        }
        if let Some(request) = self.requests.get_mut(&lead.id) {
            request.pending_final = None;
        }
        if let Some(owner) = self.agents.get_mut(&agent) {
            owner.current_request = None;
            owner.actionable_error = uncertain.map(Self::unconfirmed_submission_message);
        }
        true
    }

    fn bind_recorded_member(&mut self, id: RequestId, session: Option<String>, turn: &str) {
        if let Some(member) = self.requests.get_mut(&id) {
            member.phase = RequestPhase::Active;
            member.trusted_start_bound = true;
            member.uncertain_outcome = false;
            member.submission_error = None;
            member.provider_session_id = session;
            member.provider_turn_id = Some(turn.to_owned());
        }
    }

    /// A transcript can claim the input in a distinct turn even without its
    /// submit hook. Recover that turn's answer independently of the old lead.
    fn recover_steered_answer(
        &mut self,
        lead: &Request,
        member: &Request,
        members: &[RequestId],
        recorded: Option<RecordedTurnEnd>,
        now_ms: u64,
    ) -> bool {
        let Some(RecordedTurnEnd::Final {
            turn_id,
            text,
            prompts,
        }) = recorded
        else {
            return false;
        };
        if lead.provider_turn_id.as_deref() == Some(&turn_id) {
            return false;
        }
        // Evaluate the whole claim against the original group before changing
        // it. Spending the recorded turn for one member would hide its answer
        // from the other inputs that this very same evidence proves.
        let proved: Vec<_> = members
            .iter()
            .copied()
            .filter(|id| {
                self.requests.get(id).is_some_and(|candidate| {
                    !candidate.settled() && self.recorded_for(candidate, lead.id, members, &prompts)
                })
            })
            .collect();
        if !proved.contains(&member.id) {
            return false;
        }
        for id in proved {
            self.bind_recorded_member(id, lead.provider_session_id.clone(), &turn_id);
            if let Some(candidate) = self.requests.get_mut(&id) {
                candidate.group = (id != member.id).then_some(member.id);
            }
        }
        if let Some(owner) = self.agents.get_mut(&lead.agent_id) {
            owner.current_request = Some(member.id);
        }
        let answer = PendingFinal {
            callback_id: format!("rollout:{turn_id}"),
            text,
            received_at_ms: now_ms,
            provider_session_id: lead.provider_session_id.clone(),
            provider_turn_id: Some(turn_id),
        };
        let recovered = self
            .deliver_recorded_final(lead.agent_id, member.id, answer, &prompts, now_ms)
            .is_some();
        if let Some(owner) = self.agents.get_mut(&lead.agent_id) {
            owner.current_request = Some(lead.id);
        }
        recovered
    }

    pub(super) fn note_turn_end(&mut self, agent: AgentId, now_ms: u64) {
        let Some(agent_state) = self.agents.get(&agent).filter(|agent| !agent.dialog) else {
            return;
        };
        let Some(id) = agent_state.current_request else {
            return;
        };
        let ended = self.requests.get(&id).is_some_and(|request| {
            !request.awaiting_background
                && request.pending_final.is_none()
                && self.turn_ended(request)
        });
        if ended {
            if let Some(request) = self.requests.get_mut(&id) {
                request.turn_ended_at_ms.get_or_insert(now_ms);
            }
        }
    }

    /// A textless background pause must not block an idle orchestrator's inbox.
    pub(crate) fn release_orchestrator_inbox(
        &mut self,
        agent: AgentId,
        now_ms: u64,
    ) -> Option<RequestId> {
        let owner = self.agents.get(&agent)?;
        if owner.orchestrates.is_none()
            || owner.status != RuntimeStatus::Idle
            || owner.dialog
            || owner.deletion_pending
            || owner.session_binding_invalidated
            || !owner.hook_setup_confirmed
            || self.queued_requests(agent).is_empty()
        {
            return None;
        }
        let id = owner.current_request?;
        let request = self.requests.get(&id)?;
        if request.phase != RequestPhase::Active
            || !request.trusted_start_bound
            || !request.awaiting_background
        {
            return None;
        }
        self.close_turn_with_reason(id, now_ms, "background_wait_superseded".into())
            .ok()?;
        Some(id)
    }

    /// A typed message that starts a different turn must not replace an
    /// answer already captured for the previous turn. Move unbound steering
    /// along with the new lead, leaving genuinely folded messages behind.
    pub(super) fn split_answered_turn(
        &mut self,
        lead: RequestId,
        next: RequestId,
        callback: &super::ProviderCallback,
    ) -> Result<bool, ModelError> {
        let previous = self
            .requests
            .get(&lead)
            .ok_or(ModelError::UnknownRequest(lead))?;
        if previous.pending_final.is_none() {
            // A running generation can be replaced before it answers (Cursor
            // does this for follow-ups). Its messages still await one answer.
            return Ok(false);
        };
        let boundary = previous.submission_boundary;
        for id in self.group_members(lead) {
            if let Some(member) = self.requests.get_mut(&id).filter(|member| {
                matches!(
                    member.phase,
                    RequestPhase::Submitting | RequestPhase::Active
                ) && (member.id == next || (member.steered && !member.trusted_start_bound))
            }) {
                member.group = (id != next).then_some(next);
                member.submission_boundary = boundary;
            }
        }
        self.complete_pending_final(callback.agent_id, callback.occurred_at_ms)?;
        let member = self
            .requests
            .get_mut(&next)
            .ok_or(ModelError::UnknownRequest(next))?;
        member.phase = RequestPhase::Active;
        member.provider_session_id = callback.provider_session_id.clone();
        member.provider_turn_id = callback.provider_turn_id.clone();
        member.provider_prompt_id = callback.provider_prompt_id.clone();
        member.trusted_start_bound = true;
        member.uncertain_outcome = false;
        member.progress_at_ms = Some(callback.occurred_at_ms);
        let agent = self
            .agents
            .get_mut(&callback.agent_id)
            .ok_or(ModelError::UnknownAgent(callback.agent_id))?;
        member.submission_status_revision = agent.status_revision;
        agent.current_request = Some(next);
        Ok(true)
    }

    /// The agent's current request when its turn ended without a reply and
    /// `settle_ended_request` will close it once the settle window passes.
    /// An agent that went unavailable after the turn ended (its provider
    /// exited, say) settles too, or async waiters on it would never finish.
    fn settle_candidate(&self, agent: AgentId) -> Option<&Request> {
        let agent_state = self.agents.get(&agent)?;
        if agent_state.provider == super::Provider::Kimi
            || agent_state.deletion_pending
            || agent_state.missing_binary.is_some()
            || agent_state.session_binding_invalidated
            || agent_state.dialog
            || !matches!(
                agent_state.status,
                RuntimeStatus::Idle | RuntimeStatus::Unavailable
            )
        {
            return None;
        }
        let request = self.requests.get(&agent_state.current_request?)?;
        (!request.awaiting_background
            && request.pending_final.is_none()
            && request.turn_ended_at_ms.is_some())
        .then_some(request)
    }

    /// Whether `request` (or the turn it joined) ended without a reply and
    /// awaits settlement, which attaches its reason (a provider error, say).
    /// Followers wait for that instead of taking the turn as done.
    pub(crate) fn settling(&self, request: &Request) -> bool {
        !request.settled()
            && self
                .settle_candidate(request.agent_id)
                .is_some_and(|lead| lead.id == request.group.unwrap_or(request.id))
    }

    /// Run after callback consumption, so a final spooled at the deadline wins.
    /// `recorded_end` reads how the provider's own record says the turn ended.
    /// A final answer it attributes to this request is the reply (live,
    /// 2026-10-20: a Codex worker removed its own cwd, after which Codex ran
    /// no `Stop` hook, and two finished answers were lost). Otherwise no reply
    /// is invented; the sender sees `abandoned` with a reason:
    /// `provider_error: ...` when the record names the error that ended the
    /// turn, else `reply_not_captured`, or `agent_unavailable` when the agent
    /// is unavailable. An unavailable agent gets the window again from when it
    /// went unavailable, so one that only reattaches (a Bus restart) and is
    /// Idle again settles as usual.
    pub(crate) fn settle_ended_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        recorded_end: impl FnOnce(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        if self.agents.get(&agent)?.provider == super::Provider::Kimi {
            return self.settle_kimi_ended_request(agent, now_ms, recorded_end);
        }
        let request = self.settle_candidate(agent)?;
        let id = request.id;
        let agent_state = self.agents.get(&agent)?;
        let unavailable = agent_state.status == RuntimeStatus::Unavailable;
        let mut since = request.turn_ended_at_ms?;
        if unavailable {
            since = since.max(agent_state.status_since_ms);
        }
        if now_ms < since.saturating_add(TURN_SETTLE_MS) {
            return None;
        }
        self.consume_recorded_end(
            agent,
            id,
            now_ms,
            recorded_end(request),
            "rollout",
            unavailable,
            "Agent became unavailable after its turn ended without a captured reply; request closed. Inspect its terminal and resume it.",
            "Provider turn ended without a captured reply; request closed. Inspect its terminal for the result.",
        )
    }

    /// Kimi's idle frame is not a turn boundary. Close only after an owned
    /// terminal wire record, or a confirmed process exit with its grace window.
    fn settle_kimi_ended_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        recorded_end: impl FnOnce(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        let owner = self.agents.get(&agent)?;
        if owner.dialog
            || owner.deletion_pending
            || owner.session_binding_invalidated
            || !matches!(
                owner.status,
                RuntimeStatus::Idle | RuntimeStatus::Unavailable
            )
        {
            return None;
        }
        let request = self.requests.get(&owner.current_request?)?;
        if !matches!(
            request.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) || request.awaiting_background
            || request.pending_final.is_some()
        {
            return None;
        }
        let id = request.id;
        let exited = owner.status == RuntimeStatus::Unavailable
            && owner.exit_snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.launch_id == request.expected_launch_id
                    && now_ms >= snapshot.at_ms.saturating_add(TURN_SETTLE_MS)
            });
        if !(request.trusted_start_bound || exited && request.phase == RequestPhase::Active) {
            return None;
        }
        let recorded = recorded_end(request);
        if recorded.is_none() && !exited {
            return None;
        }
        self.consume_recorded_end(
            agent,
            id,
            now_ms,
            recorded,
            "kimi-wire",
            exited,
            "Kimi process exited without a captured reply; inspect its terminal.",
            "Kimi completed without a captured reply; inspect its terminal.",
        )
    }

    fn consume_recorded_end(
        &mut self,
        agent: AgentId,
        id: RequestId,
        now_ms: u64,
        recorded: Option<RecordedTurnEnd>,
        callback_prefix: &str,
        unavailable: bool,
        unavailable_error: &str,
        no_reply_error: &str,
    ) -> Option<RequestId> {
        let provider_error = match recorded {
            Some(RecordedTurnEnd::Final {
                turn_id,
                text,
                prompts,
            }) => {
                let answer = PendingFinal {
                    callback_id: format!("{callback_prefix}:{turn_id}"),
                    text,
                    received_at_ms: now_ms,
                    provider_session_id: None,
                    provider_turn_id: Some(turn_id),
                };
                if let Some(id) = self.deliver_recorded_final(agent, id, answer, &prompts, now_ms) {
                    return Some(id);
                }
                None
            }
            Some(RecordedTurnEnd::Error(message)) => Some(message),
            Some(RecordedTurnEnd::NoReply) | None => None,
        };
        let (reason, error) = match provider_error {
            Some(message) => provider_error_outcome(&message),
            None if unavailable => (AGENT_UNAVAILABLE.to_owned(), unavailable_error.to_owned()),
            None => ("reply_not_captured".to_owned(), no_reply_error.to_owned()),
        };
        let closed = self.close_turn_with_reason(id, now_ms, reason).ok()?;
        if let Some(owner) = self.agents.get_mut(&agent) {
            owner.actionable_error = closed.then_some(error);
        }
        Some(id)
    }

    /// Completes the agent's current request `id`, with its group, on the
    /// `answer` the provider recorded for its turn, unless a reply for that
    /// turn was already delivered. A message typed into the turn that no hook
    /// placed shares the answer only when `prompts`, the turn's recorded
    /// prompts, hold it: Codex may have run it as a turn of its own (found in
    /// review of #112), so otherwise it leaves the group as
    /// `reply_not_captured` rather than take another turn's answer.
    fn deliver_recorded_final(
        &mut self,
        agent: AgentId,
        id: RequestId,
        mut answer: PendingFinal,
        prompts: &[String],
        now_ms: u64,
    ) -> Option<RequestId> {
        let request = self.requests.get(&id)?;
        let key = provider_turn_key(
            request.expected_launch_id.as_deref().unwrap_or_default(),
            request.provider_session_id.as_deref(),
            answer.provider_turn_id.as_deref(),
        );
        if key.is_some_and(|key| self.consumed_provider_turns.contains(&key)) {
            return None;
        }
        let members = self.group_members(id);
        let unproven: Vec<RequestId> = members
            .iter()
            .filter_map(|member| self.requests.get(member))
            .filter(|member| {
                member.steered
                    && !member.trusted_start_bound
                    && !member.settled()
                    && !self.recorded_for(member, id, &members, prompts)
            })
            .map(|member| member.id)
            .collect();
        for member in unproven {
            let Some(member) = self.requests.get_mut(&member) else {
                continue;
            };
            member.group = None;
            member.phase = RequestPhase::Abandoned;
            member.completed_at_ms = Some(now_ms);
            member.failure_reason = Some("reply_not_captured".to_owned());
        }
        let session = self.requests.get(&id)?.provider_session_id.clone();
        if let Some(turn) = answer.provider_turn_id.as_deref() {
            for member in members {
                if self.requests.get(&member).is_some_and(|m| !m.settled()) {
                    self.bind_recorded_member(member, session.clone(), turn);
                }
            }
        }
        let request = self.requests.get_mut(&id)?;
        request.turn_ended_at_ms = None;
        answer.provider_session_id = request.provider_session_id.clone();
        request.pending_final = Some(answer);
        // Only hook- or transcript-proven members share the captured answer.
        self.complete_pending_final(agent, now_ms).ok()?;
        tracing::info!(
            event = "bus.message.recovered",
            request_id = id.0,
            agent_id = agent.0,
            reason = "provider_recorded_final",
            "Reply recovered from the provider's record of the turn"
        );
        Some(id)
    }

    /// Whether `prompts`, one per submission the turn recorded, hold
    /// `member`'s own: more of its text than the other requests of the group
    /// led by `lead` that share that text can account for. A repeat of the
    /// lead's text recorded once is the lead's (found in review of #112).
    fn recorded_for(
        &self,
        member: &Request,
        lead: RequestId,
        members: &[RequestId],
        prompts: &[String],
    ) -> bool {
        let own = |prompt: &&String| member.matches_callback_payload(prompt);
        let recorded = prompts.iter().filter(own).count();
        let others = std::iter::once(&lead)
            .chain(members)
            .filter(|other| **other != member.id)
            .filter_map(|other| self.requests.get(other))
            .filter(|other| {
                prompts
                    .iter()
                    .filter(own)
                    .any(|prompt| other.matches_callback_payload(prompt))
            })
            .count();
        recorded > others
    }

    /// Describe the sender's own message separately from the older turn it
    /// joined. These persisted fields also cover status after restart/archive.
    pub(crate) fn steered_turn_failure(&self, request: &Request) -> Option<String> {
        self.describe_steered_failure(request, &format!("Message {}", request.prompt.id.0), false)
    }

    /// A group notice shares one explanation among messages with the same
    /// provider reason and receipt evidence, keeping it within the queue limit.
    pub(super) fn steered_failure_notice(&self, requests: &[&Request]) -> Option<String> {
        let first = requests.first()?;
        let mut subject = requests
            .iter()
            .take(20)
            .map(|r| r.prompt.id.0.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if requests.len() > 20 {
            subject.push_str(&format!(
                ", and {} more (inspect bus history --room {})",
                requests.len() - 20,
                first.room_id.0
            ));
        }
        let label = if requests.len() == 1 {
            "Message"
        } else {
            "Messages"
        };
        self.describe_steered_failure(first, &format!("{label} {subject}"), true)
    }

    fn describe_steered_failure(
        &self,
        request: &Request,
        subject: &str,
        concise: bool,
    ) -> Option<String> {
        if !request.steered || request.phase != RequestPhase::Abandoned || request.reply().is_some()
        {
            return None;
        }
        let error = request
            .failure_reason
            .as_deref()?
            .strip_prefix("provider_error: ")?;
        let lead = self.request(request.group?)?;
        let agent = self.agent_label(request.agent_id);
        let (delivery, receipt) = if request.trusted_start_bound {
            (
                "Delivered",
                "The recipient saw your text but may not have acted on it; resend if needed.",
            )
        } else {
            (
                "Submitted",
                "Bus could not confirm whether the recipient saw your text; resend if needed.",
            )
        };
        let error = if concise {
            super::worker_events::preview(
                error,
                &format!("bus message status {}", request.prompt.id.0),
            )
        } else {
            error.to_owned()
        };
        Some(format!(
            "{subject}: {delivery} into {agent}'s running turn (started by message {}); that turn ended on a provider error: {}. {receipt}",
            lead.prompt.id.0, error.trim_end_matches('.')
        ))
    }

    /// Generic provider failure hooks (notably Cursor's) give only a display
    /// message. Preserve it for steered senders without changing the lead's
    /// established failure reason or actionable error.
    pub(super) fn note_steered_provider_error(&mut self, lead: RequestId, error: &str) {
        for member in self.group_members(lead) {
            if let Some(request) = self
                .requests
                .get_mut(&member)
                .filter(|r| r.steered && !r.settled())
            {
                request.failure_reason = Some(format!("provider_error: {error}"));
            }
        }
    }

    /// Abandon `id`'s turn with `reason`. Messages typed or coalesced into
    /// this turn close with it and share its reason; members that already
    /// settled keep their own outcome. Returns whether outstanding work closed,
    /// so caller feedback describes the requests actually affected.
    pub(super) fn close_turn_with_reason(
        &mut self,
        id: RequestId,
        now_ms: u64,
        reason: String,
    ) -> Result<bool, ModelError> {
        let detail = (reason == "submission_unconfirmed")
            .then(|| {
                self.requests
                    .get(&id)
                    .and_then(|request| request.submission_error.clone())
            })
            .flatten();
        let mut closed = self.group_members(id);

        closed.retain(|member| self.requests.get(member).is_some_and(|m| !m.settled()));
        if self.request(id).is_some_and(|r| !r.settled()) {
            closed.push(id);
        }
        let affected = !closed.is_empty();
        for request in closed {
            if let Some(request) = self.requests.get_mut(&request) {
                request.failure_reason = Some(reason.clone());
                if detail.is_some() {
                    request.submission_error = detail.clone();
                }
            }
        }
        // Retain the provider outcome before abandoning the group.
        self.abandon_current_request(id, now_ms)?;
        Ok(affected)
    }
}

/// The request reason and agent error for a turn a provider error ended:
/// `provider_error: ...` tells callers (and `message_provider_error`) to retry.
pub(super) fn provider_error_outcome(message: &str) -> (String, String) {
    (
        format!("provider_error: {message}"),
        format!("Provider turn failed: {message}; request closed. Resend it to retry."),
    )
}
