use super::{Request, RequestPhase};
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
        "interrupted" | "agent_deleted" | "room_deleted" | "background_wait_superseded" => {
            RequestFailureClass::Deliberate
        }
        "provider_error" => RequestFailureClass::Provider,
        reason if reason.starts_with("provider_error:") => RequestFailureClass::Provider,
        _ => RequestFailureClass::Bus,
    }
}

impl Request {
    /// Persisted stage shared by message status and dashboard audit records.
    pub(crate) fn delivery_stage(&self) -> &'static str {
        if self.reply().is_some() {
            return "replied";
        }
        match self.phase {
            RequestPhase::Queued => "queued",
            RequestPhase::Submitting => "submitting",
            RequestPhase::Active if self.group.is_some() => "joined",
            RequestPhase::Active if self.trusted_start_bound => "delivered",
            RequestPhase::Active => "awaiting_start",
            RequestPhase::Completed => "replied",
            RequestPhase::Abandoned => "abandoned",
        }
    }
}
