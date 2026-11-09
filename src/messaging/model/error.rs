//! Failures returned by messaging state operations.
use super::types::DEVELOPER_RECIPIENT;
use super::{AgentId, RequestId, RoomId, MASTER_ROOM_NAME};

/// Shared sentence for a send that names a retired recipient. Callers must not
/// record the refusal, and logs must not surface it as a failure.
pub(crate) const RECIPIENT_DELETED_REFUSAL: &str = "no longer exists; the message was not sent";

pub(crate) fn recipient_deleted_message(name: &str, agent: AgentId) -> String {
    format!(
        "Recipient {name} (agent {}) {RECIPIENT_DELETED_REFUSAL}",
        agent.0
    )
}

pub(crate) fn is_recipient_deleted_refusal(message: &str) -> bool {
    message.contains(RECIPIENT_DELETED_REFUSAL)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ModelError {
    InvalidName,
    UnknownRoom(RoomId),
    UnknownAgent(AgentId),
    UnknownRequest(RequestId),
    AgentOutsideRoom(AgentId),
    EmptyPrompt,
    NoRecipients,
    InvalidTransition,
    MissingLaunchIdentity,
    LaunchIdentityMismatch,
    DeletionPending,
    AgentNotIdle,
    MasterRoomFixed,
    ReservedRoomName,
    ReservedAgentName,
    AgentNameTaken {
        name: String,
        agent: AgentId,
        room: RoomId,
    },
    OrchestratorOutsideMaster(AgentId),
    NotOrchestratable(RoomId),
    RoomAlreadyOrchestrated {
        room: RoomId,
        agent: AgentId,
    },
    OrchestratorAlreadyBound(AgentId),
    /// The recipient was already deleted. Nothing was queued.
    RecipientDeleted {
        agent: AgentId,
        name: String,
    },
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MasterRoomFixed => {
                write!(formatter, "The MASTER room cannot be renamed or deleted")
            }
            Self::ReservedRoomName => write!(
                formatter,
                "The name {MASTER_ROOM_NAME} is reserved for the master room"
            ),
            Self::ReservedAgentName => write!(
                formatter,
                "The names you and {DEVELOPER_RECIPIENT} are reserved for messages to the developer"
            ),
            Self::AgentNameTaken { name, agent, room } => write!(
                formatter,
                "Agent name {name:?} is already used by agent {} in room {}; choose a unique name across all rooms",
                agent.0, room.0
            ),
            Self::OrchestratorOutsideMaster(agent) => write!(
                formatter,
                "Agent {} is not in the MASTER room; only MASTER agents orchestrate rooms",
                agent.0
            ),
            Self::NotOrchestratable(room) => write!(
                formatter,
                "Room {} cannot be orchestrated; choose an existing work room",
                room.0
            ),
            Self::RoomAlreadyOrchestrated { room, agent } => write!(
                formatter,
                "Room {} already has orchestrator agent {}; a work room has at most one orchestrator",
                room.0, agent.0
            ),
            Self::OrchestratorAlreadyBound(agent) => write!(
                formatter,
                "Agent {} already orchestrates a room; an orchestrator keeps its room for life",
                agent.0
            ),
            Self::EmptyPrompt => write!(formatter, "The message has no text or files"),
            Self::NoRecipients => write!(formatter, "Choose at least one recipient"),
            Self::DeletionPending => write!(formatter, "That room or agent is being deleted"),
            Self::RecipientDeleted { agent, name } => {
                write!(formatter, "{}", recipient_deleted_message(name, *agent))
            }
            _ => write!(formatter, "Bus model operation failed: {self:?}"),
        }
    }
}

impl std::error::Error for ModelError {}
