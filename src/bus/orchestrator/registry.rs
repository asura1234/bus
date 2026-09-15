use std::num::NonZeroU32;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::bus::model::{AgentId, RequestId};

use super::{
    AgentTerminalReadSelection, ApprovedPermissionResponse, ApprovedWorkflowPromotion,
    ExactPermissionGrant, LeaseId, OrchestratorCommand, ParticipantId, RoomMessage, RoomOperation,
    RoomQuery, RoomRecipient, WorkId, WorkflowDraftMutation,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OrchestratorToolSchema {
    pub(crate) name: &'static str,
    pub(crate) mutating: bool,
}

impl OrchestratorToolSchema {
    /// Provider-facing schema for the same closed arguments decoded below.
    /// The model never receives the internal Bus command or filesystem shape.
    pub(crate) fn provider_definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": if self.mutating {
                    "Request one already-authorized typed room operation."
                } else {
                    "Read bounded factual room evidence without changing room state."
                },
                "parameters": provider_parameters(self.name),
                "strict": true,
            }
        })
    }
}

fn provider_parameters(name: &str) -> Value {
    let integer = || json!({"type": "integer", "minimum": 0});
    let positive_integer = || json!({"type": "integer", "minimum": 1});
    let string = || json!({"type": "string", "minLength": 1});
    let recipient = || {
        json!({
            "oneOf": [
                {"type": "string", "enum": ["human", "orchestrator"]},
                {"type": "object", "properties": {"agent": integer()}, "required": ["agent"], "additionalProperties": false}
            ]
        })
    };

    let (properties, required): (Map<String, Value>, Vec<&str>) = match name {
        "inspect_work" => (map([("work_id", integer())]), vec!["work_id"]),
        "wait_for_change" => (
            map([
                ("after_revision", integer()),
                ("timeout_ms", positive_integer()),
            ]),
            vec!["after_revision", "timeout_ms"],
        ),
        "read_agent" => (
            map([
                ("agent_id", integer()),
                (
                    "source",
                    json!({"type": "string", "enum": ["visible", "recent"]}),
                ),
                ("lines", json!({"type": ["integer", "null"], "minimum": 1})),
            ]),
            vec!["agent_id", "source", "lines"],
        ),
        "observe_permission_prompt" => (map([("agent_id", integer())]), vec!["agent_id"]),
        "read_workflow_draft" => (
            map([("draft_id", string()), ("max_bytes", positive_integer())]),
            vec!["draft_id", "max_bytes"],
        ),
        "read_content" => (
            map([("kind", string()), ("name", string())]),
            vec!["kind", "name"],
        ),
        "send_message" => (
            map([
                ("to", recipient()),
                ("text", json!({"type": "string"})),
                (
                    "work_id",
                    json!({"type": ["integer", "null"], "minimum": 0}),
                ),
            ]),
            vec!["to", "text", "work_id"],
        ),
        "propose_room_brief" => (
            map([
                ("expected_revision", integer()),
                ("goal", json!({"type": "string"})),
                ("non_goals", json!({"type": "string"})),
            ]),
            vec!["expected_revision", "goal", "non_goals"],
        ),
        "abandon_idle_request" => (
            map([
                ("request_id", integer()),
                ("expected_agent", integer()),
                ("expected_incarnation", positive_integer()),
                ("expected_semantic_revision", integer()),
                ("expected_turn", json!({"type": ["string", "null"]})),
            ]),
            vec![
                "request_id",
                "expected_agent",
                "expected_incarnation",
                "expected_semantic_revision",
                "expected_turn",
            ],
        ),
        "persist_workflow_draft" => (
            map([
                ("draft_id", json!({"type": ["string", "null"]})),
                (
                    "expected_revision",
                    json!({"type": ["integer", "null"], "minimum": 1}),
                ),
                ("workflow_id", json!({"type": ["string", "null"]})),
                ("markdown", json!({"type": "string"})),
            ]),
            vec!["draft_id", "expected_revision", "workflow_id", "markdown"],
        ),
        "promote_workflow_draft" => (map([("approval_id", string())]), vec!["approval_id"]),
        "acquire_resource" => (
            map([("resource", string()), ("ttl_ms", positive_integer())]),
            vec!["resource", "ttl_ms"],
        ),
        "release_resource" => (map([("lease_id", integer())]), vec!["lease_id"]),
        "approve_permission_once" => (
            map([
                ("fingerprint", string()),
                (
                    "response",
                    json!({"type": "string", "enum": ["allow-once"]}),
                ),
            ]),
            vec!["fingerprint", "response"],
        ),
        _ => unreachable!("only closed registry names have provider schemas"),
    };
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn map<const N: usize>(entries: [(&str, Value); N]) -> Map<String, Value> {
    entries
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

#[cfg(test)]
pub(crate) const EXPECTED_TOOL_NAMES: &[&str] = &[
    "inspect_work",
    "wait_for_change",
    "read_agent",
    "observe_permission_prompt",
    "read_workflow_draft",
    "read_content",
    "send_message",
    "propose_room_brief",
    "abandon_idle_request",
    "persist_workflow_draft",
    "promote_workflow_draft",
    "acquire_resource",
    "release_resource",
    "approve_permission_once",
];

const SCHEMAS: &[OrchestratorToolSchema] = &[
    OrchestratorToolSchema {
        name: "inspect_work",
        mutating: false,
    },
    OrchestratorToolSchema {
        name: "wait_for_change",
        mutating: false,
    },
    OrchestratorToolSchema {
        name: "read_agent",
        mutating: false,
    },
    OrchestratorToolSchema {
        name: "observe_permission_prompt",
        mutating: false,
    },
    OrchestratorToolSchema {
        name: "read_workflow_draft",
        mutating: false,
    },
    OrchestratorToolSchema {
        name: "read_content",
        mutating: false,
    },
    OrchestratorToolSchema {
        name: "send_message",
        mutating: true,
    },
    OrchestratorToolSchema {
        name: "propose_room_brief",
        mutating: true,
    },
    OrchestratorToolSchema {
        name: "abandon_idle_request",
        mutating: true,
    },
    OrchestratorToolSchema {
        name: "persist_workflow_draft",
        mutating: true,
    },
    OrchestratorToolSchema {
        name: "promote_workflow_draft",
        mutating: true,
    },
    OrchestratorToolSchema {
        name: "acquire_resource",
        mutating: true,
    },
    OrchestratorToolSchema {
        name: "release_resource",
        mutating: true,
    },
    OrchestratorToolSchema {
        name: "approve_permission_once",
        mutating: true,
    },
];

#[derive(Clone, Debug)]
pub(crate) struct ModelToolCall {
    pub(crate) name: String,
    pub(crate) arguments: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ToolPolicyError {
    UnknownTool(String),
    InvalidArguments(String),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct OrchestratorToolRegistry;

impl OrchestratorToolRegistry {
    pub(crate) fn schemas(&self) -> &'static [OrchestratorToolSchema] {
        SCHEMAS
    }

    pub(crate) fn decode(
        &self,
        call: ModelToolCall,
    ) -> Result<OrchestratorCommand, ToolPolicyError> {
        fn decode<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, ToolPolicyError> {
            serde_json::from_value(value)
                .map_err(|error| ToolPolicyError::InvalidArguments(error.to_string()))
        }

        match call.name.as_str() {
            "inspect_work" => {
                let p: InspectWorkArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Query(RoomQuery::InspectWork {
                    work_id: WorkId(p.work_id),
                }))
            }
            "wait_for_change" => {
                let p: WaitArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Query(RoomQuery::WaitForChange {
                    after_revision: p.after_revision,
                    timeout_ms: p.timeout_ms,
                }))
            }
            "read_agent" => {
                let p: ReadAgentArgs = decode(call.arguments)?;
                let selection = match (p.source.as_str(), p.lines) {
                    ("visible", None) => AgentTerminalReadSelection::VisibleViewport,
                    ("recent", Some(lines)) => AgentTerminalReadSelection::RecentTail {
                        lines: NonZeroU32::new(lines).ok_or_else(|| {
                            ToolPolicyError::InvalidArguments("lines must be positive".into())
                        })?,
                    },
                    ("visible", Some(_)) => {
                        return Err(ToolPolicyError::InvalidArguments(
                            "visible does not accept lines".into(),
                        ))
                    }
                    ("recent", None) => {
                        return Err(ToolPolicyError::InvalidArguments(
                            "recent requires lines".into(),
                        ))
                    }
                    _ => return Err(ToolPolicyError::InvalidArguments("unknown source".into())),
                };
                Ok(OrchestratorCommand::Query(RoomQuery::ReadAgent {
                    agent_id: AgentId(p.agent_id),
                    selection,
                }))
            }
            "observe_permission_prompt" => {
                let p: AgentArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Query(
                    RoomQuery::ObservePermissionPrompt {
                        agent_id: AgentId(p.agent_id),
                    },
                ))
            }
            "read_workflow_draft" => {
                let p: WorkflowReadArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Query(RoomQuery::ReadWorkflowDraft {
                    draft_id: p.draft_id,
                    max_bytes: p.max_bytes,
                }))
            }
            "read_content" => {
                let p: ContentArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Query(RoomQuery::ReadContent {
                    kind: p.kind,
                    name: p.name,
                }))
            }
            "send_message" => {
                let p: MessageArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Operation(RoomOperation::SendMessage(
                    p.into_message(),
                )))
            }
            "propose_room_brief" => {
                let p: ProposeRoomBriefArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Operation(
                    RoomOperation::ProposeRoomBrief {
                        expected_revision: p.expected_revision,
                        goal: p.goal,
                        non_goals: p.non_goals,
                    },
                ))
            }
            "abandon_idle_request" => {
                let p: AbandonArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Operation(
                    RoomOperation::AbandonIdleRequest {
                        request_id: RequestId(p.request_id),
                        expected_agent: AgentId(p.expected_agent),
                        expected_incarnation: p.expected_incarnation,
                        expected_semantic_revision: p.expected_semantic_revision,
                        expected_turn: p.expected_turn,
                    },
                ))
            }
            "persist_workflow_draft" => {
                let p: WorkflowMutationArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Operation(
                    RoomOperation::PersistWorkflowDraft(WorkflowDraftMutation {
                        draft_id: p.draft_id,
                        expected_revision: p.expected_revision,
                        workflow_id: p.workflow_id,
                        markdown: p.markdown,
                    }),
                ))
            }
            "promote_workflow_draft" => {
                let p: PromotionArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Operation(
                    RoomOperation::PromoteWorkflowDraft(ApprovedWorkflowPromotion {
                        approval_id: p.approval_id,
                    }),
                ))
            }
            "acquire_resource" => {
                let p: AcquireArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Operation(
                    RoomOperation::AcquireResource {
                        resource: p.resource,
                        ttl_ms: p.ttl_ms,
                    },
                ))
            }
            "release_resource" => {
                let p: ReleaseArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Operation(
                    RoomOperation::ReleaseResource {
                        lease_id: LeaseId(p.lease_id),
                    },
                ))
            }
            "approve_permission_once" => {
                let p: ApproveArgs = decode(call.arguments)?;
                Ok(OrchestratorCommand::Operation(
                    RoomOperation::ApprovePermissionOnce(ExactPermissionGrant {
                        fingerprint: p.fingerprint,
                        response: p.response,
                    }),
                ))
            }
            _ => Err(ToolPolicyError::UnknownTool(call.name)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectWorkArgs {
    work_id: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    after_revision: u64,
    timeout_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentArgs {
    agent_id: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadAgentArgs {
    agent_id: u64,
    source: String,
    lines: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowReadArgs {
    draft_id: String,
    max_bytes: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentArgs {
    kind: String,
    name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposeRoomBriefArgs {
    expected_revision: u64,
    goal: String,
    non_goals: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AbandonArgs {
    request_id: u64,
    expected_agent: u64,
    expected_incarnation: u64,
    expected_semantic_revision: u64,
    expected_turn: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowMutationArgs {
    draft_id: Option<String>,
    expected_revision: Option<u64>,
    workflow_id: Option<String>,
    markdown: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromotionArgs {
    approval_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcquireArgs {
    resource: String,
    ttl_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseArgs {
    lease_id: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproveArgs {
    fingerprint: String,
    response: ApprovedPermissionResponse,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageArgs {
    to: RoomRecipient,
    text: String,
    work_id: Option<u64>,
}

impl MessageArgs {
    fn into_message(self) -> RoomMessage {
        RoomMessage {
            author: ParticipantId::Orchestrator,
            to: self.to,
            text: self.text,
            work: self.work_id.map(WorkId),
        }
    }
}
