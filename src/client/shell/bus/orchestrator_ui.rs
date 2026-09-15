use crate::bus::{
    model::AgentId,
    orchestrator::{ParticipantId, ProviderRequestSettlement, WorkSettlement},
    workflow_drafts::WorkflowPromotionReview,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum RecipientEntry {
    Orchestrator,
    Separator,
    AllAgents,
    Agent(AgentId),
}

/// Each action carries the exact Worker facts rendered to the Human; dispatch never re-reads
/// or recomputes them from a later snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum CoordinationAction {
    ApproveProposal { revision: u64, digest: String },
    ApproveWorkflowPromotion(Box<WorkflowPromotionReview>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SettlementReason {
    Queued,
    Launching,
    Hook,
    Active,
    Settled,
    Abandoned,
    Uncertain,
}

pub(super) fn recipient_entries(
    orchestrator_enabled: bool,
    agents: impl IntoIterator<Item = AgentId>,
) -> Vec<RecipientEntry> {
    let mut entries = Vec::new();
    if orchestrator_enabled {
        entries.push(RecipientEntry::Orchestrator);
        entries.push(RecipientEntry::Separator);
    }
    entries.push(RecipientEntry::AllAgents);
    entries.extend(agents.into_iter().map(RecipientEntry::Agent));
    entries
}

pub(super) fn all_coding_agents(agents: impl IntoIterator<Item = AgentId>) -> Vec<AgentId> {
    agents.into_iter().collect()
}

pub(super) fn settlement_reason(settlement: &WorkSettlement) -> SettlementReason {
    phase_reason(
        settlement.provider_request_settlement,
        settlement.uncertain,
        settlement.callback_lineage.trusted_start_bound,
    )
}

pub(super) fn phase_reason(
    phase: ProviderRequestSettlement,
    uncertain: bool,
    trusted_start_bound: bool,
) -> SettlementReason {
    if uncertain {
        return SettlementReason::Uncertain;
    }
    match phase {
        ProviderRequestSettlement::Queued => SettlementReason::Queued,
        ProviderRequestSettlement::Submitting => SettlementReason::Launching,
        ProviderRequestSettlement::Active if !trusted_start_bound => SettlementReason::Hook,
        ProviderRequestSettlement::Active => SettlementReason::Active,
        ProviderRequestSettlement::Completed => SettlementReason::Settled,
        ProviderRequestSettlement::Abandoned => SettlementReason::Abandoned,
    }
}

pub(super) fn settlement_label(reason: SettlementReason) -> &'static str {
    match reason {
        SettlementReason::Queued => "queued",
        SettlementReason::Launching => "launching",
        SettlementReason::Hook => "hook",
        SettlementReason::Active => "active",
        SettlementReason::Settled => "settled",
        SettlementReason::Abandoned => "abandoned",
        SettlementReason::Uncertain => "uncertain",
    }
}

pub(super) fn author_label(
    author: &ParticipantId,
    agent_name: impl Fn(AgentId) -> String,
) -> String {
    match author {
        ParticipantId::Human => "You".into(),
        ParticipantId::Orchestrator => "Orchestrator".into(),
        ParticipantId::Agent(id) => agent_name(*id),
    }
}

pub(super) fn redact_secret(secret: &str) -> String {
    if secret.is_empty() {
        return String::new();
    }
    "•".repeat(secret.chars().count().min(24))
}
