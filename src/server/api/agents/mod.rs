mod basic;
mod dialog;
mod prompt;
mod read;

use crate::server::api::errors::encode_error;

fn agent_not_ready(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_ready",
        format!("agent {target} is not an active named agent"),
    )
}

fn agent_not_found(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_found",
        format!("agent target {target} not found"),
    )
}

#[cfg(test)]
use crate::protocol::api::schema::{
    AgentDialogAnswerParams, AgentDialogChooseParams, AgentDialogChooseResult, AgentDialogKind,
    AgentDialogObservation, AgentPromptParams, AgentRenameParams, AgentSendKeysParams, AgentTarget,
    ResponseResult,
};
#[cfg(test)]
use crate::server::app::App;
#[cfg(test)]
use prompt::{
    agent_prompt_submit_delay, check_unbound_prompt_identity_and_idle, claude_input_is_empty,
    codex_composer_is_empty,
};

#[cfg(test)]
#[path = "tests/basic_test.rs"]
mod tests;

#[cfg(test)]
use bytes::Bytes;
#[cfg(test)]
use prompt::AGENT_PROMPT_SUBMIT_DELAY;
#[cfg(test)]
use std::time::Duration;
