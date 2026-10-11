use serde::Deserialize;

use super::AgentState;

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentManifest {
    pub(super) id: String,
    #[serde(rename = "version")]
    pub(super) _version: Option<String>,
    pub(super) min_engine_version: Option<u32>,
    #[serde(rename = "updated_at")]
    pub(super) _updated_at: Option<String>,
    #[serde(default)]
    pub(super) aliases: Vec<String>,
    #[serde(default)]
    pub(super) rules: Vec<ManifestRule>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestRule {
    pub(super) id: String,
    pub(super) state: Option<ManifestState>,
    #[serde(default)]
    pub(super) priority: i32,
    #[serde(default = "default_region")]
    pub(super) region: String,
    #[serde(default)]
    pub(super) visible_idle: bool,
    #[serde(default)]
    pub(super) visible_blocker: bool,
    #[serde(default)]
    pub(super) visible_working: bool,
    #[serde(default)]
    pub(super) skip_state_update: bool,
    #[serde(default)]
    pub(super) all: Vec<ManifestGate>,
    #[serde(default)]
    pub(super) any: Vec<ManifestGate>,
    #[serde(default, rename = "not")]
    pub(super) not_gate: Vec<ManifestGate>,
    #[serde(default)]
    pub(super) contains: Vec<String>,
    #[serde(default)]
    pub(super) regex: Vec<String>,
    #[serde(default)]
    pub(super) line_regex: Vec<String>,
    /// The rule is the agent's own self-update chooser, which Bus answers
    /// for agents it launched; see `AutoUpdate`.
    pub(super) auto_update: Option<ManifestAutoUpdate>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestAutoUpdate {
    pub(super) choose: String,
    pub(super) success: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct ManifestGate {
    #[serde(default)]
    pub(super) all: Vec<ManifestGate>,
    #[serde(default)]
    pub(super) any: Vec<ManifestGate>,
    #[serde(default, rename = "not")]
    pub(super) not_gate: Vec<ManifestGate>,
    #[serde(default)]
    pub(super) contains: Vec<String>,
    #[serde(default)]
    pub(super) regex: Vec<String>,
    #[serde(default)]
    pub(super) line_regex: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ManifestState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

impl From<ManifestState> for AgentState {
    fn from(value: ManifestState) -> Self {
        match value {
            ManifestState::Idle => AgentState::Idle,
            ManifestState::Working => AgentState::Working,
            ManifestState::Blocked => AgentState::Blocked,
            ManifestState::Unknown => AgentState::Unknown,
        }
    }
}

fn default_region() -> String {
    "whole_recent".to_string()
}
