use super::{agents::AGENT_DELETED, AgentId, Author, BusState, Request, RequestId, RequestPhase};

pub(super) const WORKER_NOTICE_WITHDRAWN: &str = "worker_notice_withdrawn";

pub(crate) fn request_pre_start_failure(request: &Request) -> bool {
    matches!(
        request
            .failure_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("missing_reason"),
        "submission_unconfirmed"
            | "prompt_not_shown"
            | "delivery_error"
            | "prior_request_active"
            | "not_submitted"
            | "session_hook_missing"
    )
}

pub(crate) fn request_start_bound(state: &BusState, request: &Request) -> bool {
    request.trusted_start_bound
        || (!request.uncertain_outcome
            && (request.phase == RequestPhase::Active
                || request.phase == RequestPhase::Abandoned && !request.steered)
            && request
                .group
                .and_then(|id| state.request(id))
                .is_some_and(|lead| {
                    lead.agent_id == request.agent_id
                        && !request_pre_start_failure(lead)
                        && (lead.trusted_start_bound
                            || lead.reply().is_some()
                            || lead.phase == RequestPhase::Completed)
                }))
}

// Shared responsibility for durable request closure reasons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestFailureClass {
    RetiredAutomation,
    Deliberate,
    Provider,
    Bus,
}
impl RequestFailureClass {
    pub(crate) fn is_deliberate(self) -> bool {
        matches!(self, Self::RetiredAutomation | Self::Deliberate)
    }
    pub(crate) fn is_external(self) -> bool {
        self != Self::Bus
    }
}

pub(crate) fn classify_request_failure(reason: &str) -> RequestFailureClass {
    match reason {
        "github_watch_retired"
        | "room_digest_retired"
        | "messaging_onboarding_retired"
        | "operational_notice_retired" => RequestFailureClass::RetiredAutomation,
        "interrupted"
        | "agent_deleted"
        | "room_deleted"
        | "background_wait_superseded"
        | WORKER_NOTICE_WITHDRAWN => RequestFailureClass::Deliberate,
        // Manual rescue closes stuck delivery and remains a Bus-caused failure.
        "manual_recovery" => RequestFailureClass::Bus,
        "provider_error" => RequestFailureClass::Provider,
        reason if reason.starts_with("provider_error:") => RequestFailureClass::Provider,
        _ => RequestFailureClass::Bus,
    }
}

impl BusState {
    pub(super) fn request_closure_reason<'a>(&self, id: RequestId, proposed: &'a str) -> &'a str {
        if proposed == "submission_unconfirmed"
            && self.request(id).is_some_and(|request| {
                !request.trusted_start_bound
                    && request.informational_subjects_match(|id| self.agent(id).is_none())
            })
        {
            AGENT_DELETED
        } else {
            proposed
        }
    }

    pub(super) fn unconfirmed_submission_error(
        &self,
        id: RequestId,
        fallback: String,
    ) -> Option<String> {
        let lead = self.request(id)?;
        if lead.failure_reason.as_deref() != Some(AGENT_DELETED) {
            return self.request_failure_error(id, fallback);
        }
        // The lead was an obsolete notice. A coalesced message can still need
        // an unknown-outcome error for the same native write.
        let member = self
            .group_members(id)
            .into_iter()
            .filter_map(|id| self.request(id))
            .find(|request| request.failure_reason.as_deref() == Some("submission_unconfirmed"))?;
        let error = member
            .submission_notice()
            .unwrap_or_else(|| Self::unconfirmed_submission_message(member.prompt.id));
        self.request_failure_error(member.id, error)
    }
}

impl Request {
    /// A roster notice stays relevant while any of its original subjects remains.
    pub(super) fn informational_subjects_match(
        &self,
        mut predicate: impl FnMut(AgentId) -> bool,
    ) -> bool {
        self.prompt.author == Author::Bus
            && match self.informational_subject {
                Some(subject) => predicate(subject),
                None => {
                    !self.prompt.idle_agent_ids.is_empty()
                        && self.prompt.idle_agent_ids.iter().copied().all(predicate)
                }
            }
    }

    /// Persisted stage shared by message status and dashboard audit records.
    pub(crate) fn delivery_stage(&self) -> &'static str {
        if self.reply().is_some() {
            return "replied";
        }
        match self.phase {
            RequestPhase::Queued => "queued",
            RequestPhase::Submitting => "submitting",
            // Older snapshots promoted uncertain steering to Active before
            // any receipt. Restoring one must not claim the message joined.
            RequestPhase::Active if self.uncertain_outcome && !self.trusted_start_bound => {
                "submitting"
            }
            RequestPhase::Active if self.group.is_some() => "joined",
            RequestPhase::Active if self.trusted_start_bound => "delivered",
            RequestPhase::Active => "awaiting_start",
            RequestPhase::Completed => "ended",
            RequestPhase::Abandoned => "abandoned",
        }
    }
}
