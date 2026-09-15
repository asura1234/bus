use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::bus::model::{AgentId, RequestId, RoomId};

macro_rules! numeric_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub(crate) struct $name(pub(crate) u64);
    };
}

numeric_id!(WorkId);
numeric_id!(RoomMessageId);
numeric_id!(OperationId);
numeric_id!(ArtifactId);
numeric_id!(LeaseId);

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ParticipantId {
    Human,
    Orchestrator,
    Agent(AgentId),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ConfirmRoomBriefProposal {
    pub(crate) room_id: RoomId,
    pub(crate) expected_developer: ParticipantId,
    pub(crate) expected_proposal_revision: u64,
    pub(crate) expected_proposal_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RoomBriefConfirmationReceipt {
    pub(crate) confirmation_id: OperationId,
    pub(crate) developer: ParticipantId,
    pub(crate) room_id: RoomId,
    pub(crate) approved_revision: u64,
    pub(crate) proposal_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoomRecipient {
    Human,
    Orchestrator,
    Agent(AgentId),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RoomMessage {
    pub(crate) author: ParticipantId,
    pub(crate) to: RoomRecipient,
    pub(crate) text: String,
    pub(crate) work: Option<WorkId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct HumanOrchestratorMessage {
    pub(crate) room_id: RoomId,
    pub(crate) body: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RoomMessageRecord {
    pub(crate) room_id: RoomId,
    pub(crate) message_id: RoomMessageId,
    pub(crate) message: RoomMessage,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Capability {
    InspectRoom,
    Coordinate,
    AbandonIdleRequest,
    PersistWorkflowDraft,
    PromoteWorkflowDraft,
    ApprovePermissionOnce,
    ManageResourceLease,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CapabilityGrant {
    pub(crate) room_id: RoomId,
    pub(crate) participant: ParticipantId,
    pub(crate) capability: Capability,
    pub(crate) revision: u64,
    pub(crate) active: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum AgentTerminalReadSelection {
    VisibleViewport,
    RecentTail { lines: NonZeroU32 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WorkflowDraftMutation {
    pub(crate) draft_id: Option<String>,
    pub(crate) expected_revision: Option<u64>,
    #[serde(default)]
    pub(crate) workflow_id: Option<String>,
    pub(crate) markdown: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ApprovedWorkflowPromotion {
    pub(crate) approval_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CreateDeveloperWorkflowApproval {
    pub(crate) room_id: RoomId,
    pub(crate) expected_developer: ParticipantId,
    pub(crate) draft_id: String,
    pub(crate) draft_revision: u64,
    pub(crate) content_digest: String,
    pub(crate) workflow_id: String,
    pub(crate) standard_base_digest: Option<String>,
    pub(crate) reviewed_diff_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ExactPermissionGrant {
    pub(crate) fingerprint: String,
    pub(crate) response: ApprovedPermissionResponse,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ApprovedPermissionResponse {
    AllowOnce,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderRequestSettlement {
    Queued,
    Submitting,
    Active,
    Completed,
    Abandoned,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CallbackLineage {
    pub(crate) provider_session: Option<String>,
    pub(crate) provider_turn: Option<String>,
    pub(crate) provider_prompt: Option<String>,
    pub(crate) trusted_start_bound: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WorkSettlement {
    pub(crate) room_id: RoomId,
    pub(crate) work_id: Option<WorkId>,
    pub(crate) participant: ParticipantId,
    pub(crate) message_id: RoomMessageId,
    pub(crate) request_id: RequestId,
    pub(crate) operation_id: Option<OperationId>,
    pub(crate) semantic_revision: u64,
    pub(crate) provider_request_settlement: ProviderRequestSettlement,
    pub(crate) callback_lineage: CallbackLineage,
    pub(crate) has_final_reply: bool,
    pub(crate) queue_position: Option<u64>,
    pub(crate) uncertain: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum RoomQuery {
    InspectWork {
        work_id: WorkId,
    },
    WaitForChange {
        after_revision: u64,
        timeout_ms: u64,
    },
    ReadAgent {
        agent_id: AgentId,
        selection: AgentTerminalReadSelection,
    },
    ObservePermissionPrompt {
        agent_id: AgentId,
    },
    ReadWorkflowDraft {
        draft_id: String,
        max_bytes: u32,
    },
    ReadContent {
        kind: String,
        name: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum RoomOperation {
    SendMessage(RoomMessage),
    ProposeRoomBrief {
        expected_revision: u64,
        goal: String,
        non_goals: String,
    },
    AbandonIdleRequest {
        request_id: RequestId,
        expected_agent: AgentId,
        expected_incarnation: u64,
        expected_semantic_revision: u64,
        expected_turn: Option<String>,
    },
    PersistWorkflowDraft(WorkflowDraftMutation),
    PromoteWorkflowDraft(ApprovedWorkflowPromotion),
    AcquireResource {
        resource: String,
        ttl_ms: u64,
    },
    ReleaseResource {
        lease_id: LeaseId,
    },
    ApprovePermissionOnce(ExactPermissionGrant),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum OrchestratorCommand {
    Query(RoomQuery),
    Operation(RoomOperation),
}
