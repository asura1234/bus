use serde::{Deserialize, Serialize};

/// Request obligations for the exact native pane observed by the coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDetectionContext {
    pub terminal_id: String,
    pub pane_id: String,
    pub session_id: Option<String>,
    pub obligation: bool,
    pub activity_age_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentDetectionContexts {
    pub agents: Vec<AgentDetectionContext>,
}
