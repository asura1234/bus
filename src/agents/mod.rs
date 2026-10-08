//! Agent state detection via terminal tail pattern matching.
//!
//! Each pane's live bottom-of-buffer text is read periodically and matched
//! against known agent output patterns to determine state.

pub(crate) mod dialog;
pub mod manifest;
pub(crate) mod resume;
pub(crate) mod title;

#[cfg(test)]
#[path = "tests/codex_activity_test.rs"]
mod codex_activity_tests;

mod catalog;
mod detect;
mod identify;
#[cfg(test)]
pub use identify::identify_agent;

pub(crate) use self::catalog::parse_canonical_agent_label;
pub use self::catalog::{agent_label, interactive_agent_executable, parse_agent_label, Agent};
pub(crate) use self::detect::session_identity_only_integration;
pub use self::detect::{
    detect_agent_with_osc, should_skip_state_update, AgentDetection, AgentState,
};
pub use self::identify::{
    foreground_group_leader_job, foreground_job, foreground_process_group_id, identify_agent_in_job,
};

#[cfg(test)]
mod tests {
    use super::identify::{cmdline_argv0_agent_name, wrapped_agent_name_from_runtime_argv};
    include!("catalog/tests/catalog_test.rs");
    include!("identify/tests/identity_test.rs");
    include!("detect/tests/detect_test.rs");
}

pub(crate) mod providers;
pub(crate) use self::identify::process_agent_hint;
