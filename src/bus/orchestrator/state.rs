use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::bus::model::{RequestId, RoomId};

use super::{
    Capability, CapabilityGrant, LeaseId, OperationId, ParticipantId, RoomBriefConfirmationReceipt,
    RoomMessageId, RoomMessageRecord,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum JournalFact {
    HumanMessage {
        message_id: RoomMessageId,
    },
    WorkChanged {
        work_id: super::WorkId,
        fact_digest: String,
    },
    RequestSettled {
        request_id: RequestId,
        message_id: RoomMessageId,
        participant: ParticipantId,
        settlement: super::ProviderRequestSettlement,
        reply_digest: Option<String>,
    },
    OperationSettled {
        operation_id: OperationId,
    },
    RoomBriefConfirmed {
        confirmation_id: OperationId,
    },
    WorkflowApprovalCreated {
        approval_id: String,
    },
    ArtifactChanged {
        artifact_id: super::ArtifactId,
        digest: String,
    },
    ProviderActivity {
        participant: ParticipantId,
        turn: Option<String>,
        digest: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct JournalEntry {
    pub(crate) revision: u64,
    pub(crate) room_id: RoomId,
    pub(crate) fact: JournalFact,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum OperationPhase {
    IntentPersisted,
    Uncertain,
    Settled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum OperationResult {
    Applied { receipt_digest: String },
    Rejected { code: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct DurableOperation {
    pub(crate) operation_id: OperationId,
    pub(crate) room_id: RoomId,
    pub(crate) actor: ParticipantId,
    pub(crate) kind: String,
    pub(crate) intent_digest: String,
    pub(crate) phase: OperationPhase,
    pub(crate) uncertainty: Option<String>,
    pub(crate) result: Option<OperationResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct OperationIntent {
    pub(crate) operation_id: OperationId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StateError {
    #[cfg(test)]
    HumanAuthorityRequired,
    #[cfg(test)]
    StaleRevision,
    DuplicateUnsettledOperation,
    UnknownOperation,
    InvalidTransition,
    ResourceBusy,
    UnknownLease,
    WrongLeaseOwner,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ResourceLease {
    pub(crate) lease_id: LeaseId,
    pub(crate) room_id: RoomId,
    pub(crate) owner: ParticipantId,
    pub(crate) resource: String,
    pub(crate) expires_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ContextProjection {
    pub(crate) semantic_revision: u64,
    pub(crate) facts: Vec<JournalFact>,
    pub(crate) messages: Vec<RoomMessageRecord>,
    /// Workflow progression is intentionally never a runtime projection.
    pub(crate) next_action: Option<()>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct OrchestratorState {
    #[serde(default)]
    journal: Vec<JournalEntry>,
    #[serde(default)]
    operations: BTreeMap<OperationId, DurableOperation>,
    #[serde(default, deserialize_with = "deserialize_grants")]
    grants: Vec<CapabilityGrant>,
    #[serde(default)]
    leases: BTreeMap<LeaseId, ResourceLease>,
    #[serde(default)]
    workflow_drafts: crate::bus::workflow_drafts::WorkflowDraftLedger,
    #[serde(default)]
    room_brief_confirmations: BTreeMap<RoomId, RoomBriefConfirmationReceipt>,
    #[serde(default)]
    room_messages: Vec<RoomMessageRecord>,
    #[serde(default = "initial_id")]
    next_id: u64,
}

fn initial_id() -> u64 {
    1
}

/// The Orchestrator column of the role/capability contract, activated by Room Brief confirmation.
const ORCHESTRATOR_BASELINE: [Capability; 7] = [
    Capability::InspectRoom,
    Capability::Coordinate,
    Capability::AbandonIdleRequest,
    Capability::PersistWorkflowDraft,
    Capability::PromoteWorkflowDraft,
    Capability::ApprovePermissionOnce,
    Capability::ManageResourceLease,
];

fn deserialize_grants<'de, D>(deserializer: D) -> Result<Vec<CapabilityGrant>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Grants {
        Current(Vec<CapabilityGrant>),
        LegacyEmpty(BTreeMap<String, CapabilityGrant>),
    }

    match Grants::deserialize(deserializer)? {
        Grants::Current(grants) => Ok(grants),
        Grants::LegacyEmpty(grants) if grants.is_empty() => Ok(Vec::new()),
        Grants::LegacyEmpty(_) => Err(serde::de::Error::custom(
            "legacy orchestrator grants must be empty",
        )),
    }
}

impl OrchestratorState {
    fn allocate_id(&mut self) -> u64 {
        if self.next_id == 0 {
            self.next_id = 1;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    pub(crate) fn record_fact(&mut self, room_id: RoomId, fact: JournalFact) -> JournalEntry {
        if let Some(existing) = self
            .journal
            .iter()
            .find(|entry| entry.room_id == room_id && entry.fact == fact)
        {
            return existing.clone();
        }
        let revision = self
            .journal
            .iter()
            .filter(|entry| entry.room_id == room_id)
            .map(|entry| entry.revision)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let entry = JournalEntry {
            revision,
            room_id,
            fact,
        };
        self.journal.push(entry.clone());
        entry
    }

    pub(crate) fn context(&self, room_id: RoomId) -> ContextProjection {
        let entries = self.journal.iter().filter(|entry| entry.room_id == room_id);
        let facts = entries
            .clone()
            .map(|entry| entry.fact.clone())
            .collect::<Vec<_>>();
        ContextProjection {
            semantic_revision: entries.map(|entry| entry.revision).max().unwrap_or(0),
            facts,
            messages: self.room_messages(room_id).cloned().collect(),
            next_action: None,
        }
    }

    pub(crate) fn record_room_message(&mut self, record: RoomMessageRecord) {
        self.room_messages.push(record);
    }

    pub(crate) fn room_messages(
        &self,
        room_id: RoomId,
    ) -> impl Iterator<Item = &RoomMessageRecord> {
        self.room_messages
            .iter()
            .filter(move |record| record.room_id == room_id)
    }

    pub(crate) fn begin_operation(
        &mut self,
        room_id: RoomId,
        actor: ParticipantId,
        kind: &str,
        intent_digest: &str,
    ) -> Result<OperationIntent, StateError> {
        if self.operations.values().any(|operation| {
            operation.room_id == room_id
                && operation.actor == actor
                && operation.kind == kind
                && operation.intent_digest == intent_digest
                && operation.phase != OperationPhase::Settled
        }) {
            return Err(StateError::DuplicateUnsettledOperation);
        }
        let operation_id = OperationId(self.allocate_id());
        self.operations.insert(
            operation_id,
            DurableOperation {
                operation_id,
                room_id,
                actor,
                kind: kind.into(),
                intent_digest: intent_digest.into(),
                phase: OperationPhase::IntentPersisted,
                uncertainty: None,
                result: None,
            },
        );
        Ok(OperationIntent { operation_id })
    }

    #[cfg(test)]
    pub(crate) fn operation(&self, operation_id: OperationId) -> Option<&DurableOperation> {
        self.operations.get(&operation_id)
    }

    #[cfg(test)]
    pub(crate) fn operations(&self) -> impl Iterator<Item = &DurableOperation> {
        self.operations.values()
    }

    pub(crate) fn workflow_drafts_mut(
        &mut self,
    ) -> &mut crate::bus::workflow_drafts::WorkflowDraftLedger {
        &mut self.workflow_drafts
    }

    pub(crate) fn workflow_drafts(&self) -> &crate::bus::workflow_drafts::WorkflowDraftLedger {
        &self.workflow_drafts
    }

    pub(crate) fn room_brief_confirmation(
        &self,
        room_id: RoomId,
    ) -> Option<&RoomBriefConfirmationReceipt> {
        self.room_brief_confirmations.get(&room_id)
    }

    pub(crate) fn insert_room_brief_confirmation(
        &mut self,
        room_id: RoomId,
        developer: ParticipantId,
        approved_revision: u64,
        proposal_digest: String,
    ) -> RoomBriefConfirmationReceipt {
        let receipt = RoomBriefConfirmationReceipt {
            confirmation_id: OperationId(self.allocate_id()),
            developer,
            room_id,
            approved_revision,
            proposal_digest,
        };
        self.room_brief_confirmations
            .insert(room_id, receipt.clone());
        receipt
    }

    pub(crate) fn wake_revision(&self, room_id: RoomId) -> u64 {
        self.journal
            .iter()
            .filter(|entry| {
                entry.room_id == room_id
                    && !matches!(entry.fact, JournalFact::ProviderActivity { .. })
            })
            .map(|entry| entry.revision)
            .max()
            .unwrap_or(0)
    }

    pub(crate) fn has_operation_intent(&self, room_id: RoomId, kind: &str, digest: &str) -> bool {
        self.operations.values().any(|operation| {
            operation.room_id == room_id
                && operation.kind == kind
                && operation.intent_digest == digest
        })
    }

    pub(crate) fn mark_operation_uncertain(
        &mut self,
        operation_id: OperationId,
        reason: &str,
    ) -> Result<(), StateError> {
        let operation = self
            .operations
            .get_mut(&operation_id)
            .ok_or(StateError::UnknownOperation)?;
        if operation.phase != OperationPhase::IntentPersisted {
            return Err(StateError::InvalidTransition);
        }
        operation.phase = OperationPhase::Uncertain;
        operation.uncertainty = Some(reason.into());
        Ok(())
    }

    pub(crate) fn reconcile_operation(
        &mut self,
        operation_id: OperationId,
        result: OperationResult,
    ) -> Result<(), StateError> {
        let room_id = {
            let operation = self
                .operations
                .get_mut(&operation_id)
                .ok_or(StateError::UnknownOperation)?;
            if operation.phase == OperationPhase::Settled {
                return Err(StateError::InvalidTransition);
            }
            operation.phase = OperationPhase::Settled;
            operation.result = Some(result);
            operation.room_id
        };
        self.record_fact(room_id, JournalFact::OperationSettled { operation_id });
        Ok(())
    }

    pub(crate) fn authorized(
        &self,
        room_id: RoomId,
        participant: ParticipantId,
        capability: Capability,
    ) -> bool {
        participant == ParticipantId::Human
            || self
                .grants
                .iter()
                .find(|grant| {
                    grant.room_id == room_id
                        && grant.participant == participant
                        && grant.capability == capability
                })
                .is_some_and(|grant| grant.active)
    }

    /// Human Room Brief confirmation is the only production grant path.
    pub(crate) fn activate_orchestrator_baseline(&mut self, room_id: RoomId) {
        for capability in ORCHESTRATOR_BASELINE {
            self.activate_grant(room_id, ParticipantId::Orchestrator, capability);
        }
    }

    #[cfg(test)]
    pub(crate) fn grant(
        &mut self,
        room_id: RoomId,
        actor: ParticipantId,
        participant: ParticipantId,
        capability: Capability,
    ) -> Result<u64, StateError> {
        if actor != ParticipantId::Human {
            return Err(StateError::HumanAuthorityRequired);
        }
        Ok(self.activate_grant(room_id, participant, capability))
    }

    /// An already active grant keeps its revision, so activation is idempotent.
    fn activate_grant(
        &mut self,
        room_id: RoomId,
        participant: ParticipantId,
        capability: Capability,
    ) -> u64 {
        if let Some(grant) = self.grants.iter_mut().find(|grant| {
            grant.room_id == room_id
                && grant.participant == participant
                && grant.capability == capability
        }) {
            if !grant.active {
                grant.revision = grant.revision.saturating_add(1);
                grant.active = true;
            }
            return grant.revision;
        }
        self.grants.push(CapabilityGrant {
            room_id,
            participant,
            capability,
            revision: 1,
            active: true,
        });
        1
    }

    #[cfg(test)]
    pub(crate) fn revoke(
        &mut self,
        room_id: RoomId,
        actor: ParticipantId,
        participant: ParticipantId,
        capability: Capability,
        expected_revision: u64,
    ) -> Result<(), StateError> {
        if actor != ParticipantId::Human {
            return Err(StateError::HumanAuthorityRequired);
        }
        let grant = self
            .grants
            .iter_mut()
            .find(|grant| {
                grant.room_id == room_id
                    && grant.participant == participant
                    && grant.capability == capability
            })
            .ok_or(StateError::StaleRevision)?;
        if grant.revision != expected_revision {
            return Err(StateError::StaleRevision);
        }
        grant.revision = grant.revision.saturating_add(1);
        grant.active = false;
        Ok(())
    }

    pub(crate) fn acquire_resource(
        &mut self,
        room_id: RoomId,
        owner: ParticipantId,
        resource: &str,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<ResourceLease, StateError> {
        if self.leases.values().any(|lease| {
            lease.room_id == room_id && lease.resource == resource && lease.expires_at_ms > now_ms
        }) {
            return Err(StateError::ResourceBusy);
        }
        self.leases.retain(|_, lease| lease.expires_at_ms > now_ms);
        let lease = ResourceLease {
            lease_id: LeaseId(self.allocate_id()),
            room_id,
            owner,
            resource: resource.into(),
            expires_at_ms: now_ms.saturating_add(ttl_ms),
        };
        self.leases.insert(lease.lease_id, lease.clone());
        Ok(lease)
    }

    pub(crate) fn release_resource(
        &mut self,
        room_id: RoomId,
        owner: ParticipantId,
        lease_id: LeaseId,
    ) -> Result<(), StateError> {
        let lease = self.leases.get(&lease_id).ok_or(StateError::UnknownLease)?;
        if lease.room_id != room_id || lease.owner != owner {
            return Err(StateError::WrongLeaseOwner);
        }
        self.leases.remove(&lease_id);
        Ok(())
    }
}
