//! Prompt admission deadlines, activity observation and settled-state waiting.
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::wait::{
    agent_from_response, agent_wait_identity_matches, agent_wait_matches, agent_wait_not_running,
    agent_wait_statuses, wait_for_resolved_agent, AgentWaitOutcome, AgentWaitTimeoutKind,
    ResolvedAgentWait,
};
use crate::platform::ipc::LocalStream;
use crate::protocol::api::schema::{
    ErrorResponse, Method, Request, ResponseResult, SuccessResponse,
};
use crate::server::api::socket::{
    dispatch_to_app_with_caller_timeout, dispatch_to_app_with_timeout, APP_RESPONSE_TIMEOUT,
};
use crate::server::api::{ApiRequestSender, EventHub};

const AGENT_PROMPT_EFFECT_TIMEOUT_MS: u64 = 5_000;

pub(in crate::server::api) fn prompt_agent(
    request_id: String,
    mut params: crate::protocol::api::schema::AgentPromptParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    let Some(wait) = params.wait.clone() else {
        return Ok(Some(dispatch_to_app_with_timeout(
            Request {
                id: request_id,
                method: Method::AgentPrompt(params),
            },
            api_tx,
            None,
        )));
    };

    let wait_started = std::time::Instant::now();
    let before_prompt = match agent_get_for_prompt(
        &request_id,
        &params.target,
        api_tx,
        wait.timeout_ms,
        wait_started,
    ) {
        Ok(agent) => agent,
        Err(response) => {
            return serde_json::to_string(&response)
                .map(Some)
                .map_err(std::io::Error::other);
        }
    };
    let prompt_started_working =
        before_prompt.agent_status == crate::protocol::api::schema::AgentStatus::Working;
    let target = params.target.clone();
    if let Some(prompt_wait) = params.wait.as_mut() {
        prompt_wait.submission_deadline = wait
            .timeout_ms
            .map(|timeout_ms| wait_started + std::time::Duration::from_millis(timeout_ms));
    }
    let last_event_sequence = event_hub.current_sequence();
    let prompt_request = Request {
        id: request_id.clone(),
        method: Method::AgentPrompt(params),
    };
    #[cfg(windows)]
    let prompt_response = dispatch_to_app_with_caller_timeout(
        prompt_request,
        api_tx,
        remaining_timeout_ms(wait.timeout_ms, wait_started).map(std::time::Duration::from_millis),
    );
    #[cfg(not(windows))]
    let prompt_response = dispatch_to_app_with_timeout(prompt_request, api_tx, None);
    let Ok(prompted) = agent_from_response(&request_id, &prompt_response) else {
        return Ok(Some(prompt_response));
    };
    if !agent_wait_identity_matches(
        &prompted,
        &before_prompt.terminal_id,
        before_prompt.name.as_deref().filter(|name| *name == target),
        before_prompt.agent.as_deref(),
    ) {
        return agent_wait_not_running(request_id).map(Some);
    }

    let prompt_activity_observed = prompt_started_working
        || matches!(
            prompted.agent_status,
            crate::protocol::api::schema::AgentStatus::Working
                | crate::protocol::api::schema::AgentStatus::Blocked
        );
    let prompt_state_change_seq = prompted.state_change_seq;
    let until = agent_wait_statuses(wait.until);
    let mut initial = prompted;

    if !prompt_activity_observed {
        let remaining_timeout_ms = remaining_timeout_ms(wait.timeout_ms, wait_started);
        let (effect_timeout_ms, timeout_kind) = match remaining_timeout_ms {
            Some(timeout_ms) if timeout_ms <= AGENT_PROMPT_EFFECT_TIMEOUT_MS => {
                (timeout_ms, AgentWaitTimeoutKind::Status)
            }
            _ => (
                AGENT_PROMPT_EFFECT_TIMEOUT_MS,
                AgentWaitTimeoutKind::PromptStalled {
                    timeout_ms: AGENT_PROMPT_EFFECT_TIMEOUT_MS,
                },
            ),
        };
        let Some(outcome) = wait_for_resolved_agent(
            request_id.clone(),
            ResolvedAgentWait {
                target: target.clone(),
                until: prompt_activity_statuses(),
                timeout_ms: Some(effect_timeout_ms),
                initial,
                last_event_sequence,
                after_state_change_seq: Some(prompt_state_change_seq),
                accept_transient_status: true,
                timeout_kind,
            },
            stream,
            api_tx,
            event_hub,
            running,
        )?
        else {
            return Ok(None);
        };
        initial = match outcome {
            AgentWaitOutcome::Matched(agent) => *agent,
            AgentWaitOutcome::Response(response) => return Ok(Some(response)),
        };
    }
    if agent_wait_matches(&initial, &until, None) {
        return agent_prompt_success(request_id, initial).map(Some);
    }

    let Some(outcome) = wait_for_resolved_agent(
        request_id.clone(),
        ResolvedAgentWait {
            target,
            until,
            timeout_ms: remaining_timeout_ms(wait.timeout_ms, wait_started),
            initial,
            // Replay from before submission so terminal lifecycle events consumed by
            // the activity gate still terminate this settled-state wait.
            last_event_sequence,
            after_state_change_seq: None,
            accept_transient_status: false,
            timeout_kind: AgentWaitTimeoutKind::Status,
        },
        stream,
        api_tx,
        event_hub,
        running,
    )?
    else {
        return Ok(None);
    };
    let agent = match outcome {
        AgentWaitOutcome::Matched(agent) => *agent,
        AgentWaitOutcome::Response(response) => return Ok(Some(response)),
    };
    agent_prompt_success(request_id, agent).map(Some)
}

fn remaining_timeout_ms(total_ms: Option<u64>, started: std::time::Instant) -> Option<u64> {
    total_ms.map(|total_ms| {
        let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        total_ms.saturating_sub(elapsed_ms)
    })
}

fn agent_prompt_success(
    request_id: String,
    agent: crate::protocol::api::schema::AgentInfo,
) -> std::io::Result<String> {
    serde_json::to_string(&SuccessResponse {
        id: request_id,
        result: ResponseResult::AgentPrompted { agent },
    })
    .map_err(std::io::Error::other)
}

fn prompt_activity_statuses() -> Vec<crate::protocol::api::schema::AgentStatus> {
    vec![
        crate::protocol::api::schema::AgentStatus::Working,
        crate::protocol::api::schema::AgentStatus::Blocked,
    ]
}

fn agent_get_for_prompt(
    request_id: &str,
    target: &str,
    api_tx: &ApiRequestSender,
    total_timeout_ms: Option<u64>,
    started: std::time::Instant,
) -> Result<crate::protocol::api::schema::AgentInfo, ErrorResponse> {
    let request = Request {
        id: format!("{request_id}:agent"),
        method: Method::AgentGet(crate::protocol::api::schema::AgentTarget {
            target: target.to_string(),
        }),
    };
    let remaining_ms = remaining_timeout_ms(total_timeout_ms, started);
    let response = match remaining_ms {
        Some(timeout_ms) if timeout_ms <= APP_RESPONSE_TIMEOUT.as_millis() as u64 => {
            dispatch_to_app_with_caller_timeout(
                request,
                api_tx,
                Some(std::time::Duration::from_millis(timeout_ms)),
            )
        }
        _ => dispatch_to_app_with_timeout(request, api_tx, Some(APP_RESPONSE_TIMEOUT)),
    };
    agent_from_response(request_id, &response)
}
