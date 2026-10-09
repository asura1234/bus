//! Stream handlers remain visible to the sibling socket dispatcher within server::api.
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use regex::Regex;

pub(in crate::server::api) use super::prompt_wait::prompt_agent;

use crate::platform::ipc::LocalStream;
use crate::protocol::api::schema::{
    ErrorBody, ErrorResponse, EventData, EventEnvelope, EventKind, EventMatch, EventsWaitParams,
    Method, Request, ResponseResult, Subscription, SubscriptionEventData,
    SubscriptionEventEnvelope, SuccessResponse,
};
use crate::server::api::socket::{
    dispatch_to_app_with_timeout, should_stop_connection, APP_RESPONSE_TIMEOUT,
    CONNECTION_POLL_INTERVAL,
};
use crate::server::api::streams::subscriptions::ActiveSubscription;
use crate::server::api::streams::subscriptions::{match_output, output_match_read_source};
use crate::server::api::{ApiRequestSender, EventHub};

pub(in crate::server::api) fn wait_for_output(
    request_id: String,
    params: crate::protocol::api::schema::PaneWaitForOutputParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    crate::utils::logging::api_wait_started(&request_id, &params.pane_id, params.timeout_ms);
    let deadline = params
        .timeout_ms
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));

    let regex = match &params.r#match {
        crate::protocol::api::schema::OutputMatch::Regex { value } => match Regex::new(value) {
            Ok(regex) => Some(regex),
            Err(err) => {
                return Ok(Some(crate::server::api::errors::encode_error(
                    request_id,
                    "invalid_regex",
                    err.to_string(),
                )));
            }
        },
        crate::protocol::api::schema::OutputMatch::Substring { .. } => None,
    };

    loop {
        if should_stop_connection(stream, running)? {
            crate::utils::logging::api_wait_completed(
                &request_id,
                &params.pane_id,
                "client_disconnected",
            );
            return Ok(None);
        }

        let read_request = Request {
            id: format!("{request_id}:read"),
            method: Method::PaneRead(crate::protocol::api::schema::PaneReadParams {
                pane_id: params.pane_id.clone(),
                source: output_match_read_source(&params.source),
                lines: params.lines,
                format: crate::protocol::api::schema::ReadFormat::Text,
                strip_ansi: params.strip_ansi,
                intent: crate::protocol::api::schema::ReadIntent::Passive,
            }),
        };
        let response =
            dispatch_to_app_with_timeout(read_request, api_tx, Some(APP_RESPONSE_TIMEOUT));
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&response) else {
            return Ok(Some(response));
        };
        if value.get("error").is_some() {
            let mut value = value;
            value["id"] = serde_json::Value::String(request_id.clone());
            return Ok(Some(
                serde_json::to_string(&value).map_err(std::io::Error::other)?,
            ));
        }

        let read_value = value["result"]["read"].clone();
        let Ok(read) =
            serde_json::from_value::<crate::protocol::api::schema::PaneReadResult>(read_value)
        else {
            return Ok(Some(
                serde_json::to_string(&ErrorResponse {
                    id: request_id,
                    error: ErrorBody {
                        code: "internal_error".into(),
                        message: "failed to decode pane read result".into(),
                    },
                })
                .map_err(std::io::Error::other)?,
            ));
        };

        let matched_line = match_output(&read.text, &params.r#match, regex.as_ref());
        if matched_line.is_some() {
            let revision = read.revision;
            crate::utils::logging::api_wait_completed(&request_id, &params.pane_id, "matched");
            return Ok(Some(
                serde_json::to_string(&SuccessResponse {
                    id: request_id,
                    result: ResponseResult::OutputMatched {
                        pane_id: read.pane_id.clone(),
                        revision,
                        matched_line,
                        read,
                    },
                })
                .map_err(std::io::Error::other)?,
            ));
        }

        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            crate::utils::logging::api_wait_timed_out(&request_id, &params.pane_id);
            return Ok(Some(
                serde_json::to_string(&ErrorResponse {
                    id: request_id,
                    error: ErrorBody {
                        code: "timeout".into(),
                        message: "timed out waiting for output match".into(),
                    },
                })
                .map_err(std::io::Error::other)?,
            ));
        }

        std::thread::sleep(CONNECTION_POLL_INTERVAL);
    }
}

pub(in crate::server::api) fn wait_for_agent(
    request_id: String,
    params: crate::protocol::api::schema::AgentWaitParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    let last_event_sequence = event_hub.current_sequence();
    let initial = match agent_get(&request_id, &params.target, api_tx) {
        Ok(agent) => agent,
        Err(response) => {
            return serde_json::to_string(&response)
                .map(Some)
                .map_err(std::io::Error::other);
        }
    };
    let until = agent_wait_statuses(params.until);
    if agent_wait_matches(&initial, &until, None) {
        return agent_wait_success(request_id, initial).map(Some);
    }

    match wait_for_resolved_agent(
        request_id.clone(),
        ResolvedAgentWait {
            target: params.target,
            until,
            timeout_ms: params.timeout_ms,
            initial,
            last_event_sequence,
            after_state_change_seq: None,
            accept_transient_status: true,
            timeout_kind: AgentWaitTimeoutKind::Status,
        },
        stream,
        api_tx,
        event_hub,
        running,
    )? {
        Some(AgentWaitOutcome::Matched(agent)) => agent_wait_success(request_id, *agent).map(Some),
        Some(AgentWaitOutcome::Response(response)) => Ok(Some(response)),
        None => Ok(None),
    }
}

pub(super) struct ResolvedAgentWait {
    pub(super) target: String,
    pub(super) until: Vec<crate::protocol::api::schema::AgentStatus>,
    pub(super) timeout_ms: Option<u64>,
    pub(super) initial: crate::protocol::api::schema::AgentInfo,
    pub(super) last_event_sequence: u64,
    pub(super) after_state_change_seq: Option<u64>,
    pub(super) accept_transient_status: bool,
    pub(super) timeout_kind: AgentWaitTimeoutKind,
}

#[derive(Clone, Copy)]
pub(super) enum AgentWaitTimeoutKind {
    Status,
    PromptStalled { timeout_ms: u64 },
}

pub(super) enum AgentWaitOutcome {
    Matched(Box<crate::protocol::api::schema::AgentInfo>),
    Response(String),
}

pub(super) fn wait_for_resolved_agent(
    request_id: String,
    wait: ResolvedAgentWait,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<AgentWaitOutcome>> {
    let deadline = wait
        .timeout_ms
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
    let expected_terminal_id = wait.initial.terminal_id.clone();
    let expected_name = wait
        .initial
        .name
        .as_ref()
        .filter(|name| name.as_str() == wait.target)
        .cloned();
    let expected_agent = wait.initial.agent.clone();
    let pane_id = wait.initial.pane_id.clone();
    let mut last_event_sequence = wait.last_event_sequence;

    loop {
        if should_stop_connection(stream, running)? {
            return Ok(None);
        }

        let (should_probe, matched_event_status) = match scan_agent_wait_events(
            &request_id,
            &wait,
            &pane_id,
            &expected_agent,
            &mut last_event_sequence,
            event_hub,
        )? {
            AgentWaitEventScan::Outcome(outcome) => return Ok(Some(outcome)),
            AgentWaitEventScan::Probe {
                should_probe,
                matched_event_status,
            } => (should_probe, matched_event_status),
        };

        if should_probe {
            let current = match agent_get(&request_id, &wait.target, api_tx) {
                Ok(agent) => agent,
                Err(response) => {
                    return agent_wait_probe_error(response)
                        .map(AgentWaitOutcome::Response)
                        .map(Some);
                }
            };
            if !agent_wait_identity_matches(
                &current,
                &expected_terminal_id,
                expected_name.as_deref(),
                expected_agent.as_deref(),
            ) {
                return agent_wait_not_running(request_id)
                    .map(AgentWaitOutcome::Response)
                    .map(Some);
            }
            if let Some(status) = matched_event_status {
                let mut matched = current;
                matched.agent_status = status;
                return Ok(Some(AgentWaitOutcome::Matched(Box::new(matched))));
            }
            if agent_wait_matches(&current, &wait.until, wait.after_state_change_seq) {
                return Ok(Some(AgentWaitOutcome::Matched(Box::new(current))));
            }
        }

        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            let current = match agent_get(&request_id, &wait.target, api_tx) {
                Ok(agent) => agent,
                Err(response) => {
                    return agent_wait_probe_error(response)
                        .map(AgentWaitOutcome::Response)
                        .map(Some);
                }
            };
            if !agent_wait_identity_matches(
                &current,
                &expected_terminal_id,
                expected_name.as_deref(),
                expected_agent.as_deref(),
            ) {
                return agent_wait_not_running(request_id)
                    .map(AgentWaitOutcome::Response)
                    .map(Some);
            }
            if agent_wait_matches(&current, &wait.until, wait.after_state_change_seq) {
                return Ok(Some(AgentWaitOutcome::Matched(Box::new(current))));
            }
            return agent_wait_timeout(request_id, wait.timeout_kind, &current)
                .map(AgentWaitOutcome::Response)
                .map(Some);
        }
        std::thread::sleep(CONNECTION_POLL_INTERVAL);
    }
}

enum AgentWaitEventScan {
    Outcome(AgentWaitOutcome),
    Probe {
        should_probe: bool,
        matched_event_status: Option<crate::protocol::api::schema::AgentStatus>,
    },
}

fn scan_agent_wait_events(
    request_id: &str,
    wait: &ResolvedAgentWait,
    pane_id: &str,
    expected_agent: &Option<String>,
    last_event_sequence: &mut u64,
    event_hub: &EventHub,
) -> std::io::Result<AgentWaitEventScan> {
    let mut should_probe = false;
    let mut matched_event_status = None;
    for (sequence, event) in event_hub.events_after(*last_event_sequence) {
        *last_event_sequence = sequence;
        match event.data {
            EventData::PaneAgentDetected {
                pane_id: event_pane,
                agent,
                released,
                final_status,
                ..
            } if event_pane == pane_id => {
                if released {
                    if let Some(status) = final_status
                        .filter(|status| wait.until.contains(status))
                        .or(matched_event_status)
                    {
                        let mut matched = wait.initial.clone();
                        matched.agent_status = status;
                        return Ok(AgentWaitEventScan::Outcome(AgentWaitOutcome::Matched(
                            Box::new(matched),
                        )));
                    }
                    return agent_wait_not_running(request_id.to_owned())
                        .map(AgentWaitOutcome::Response)
                        .map(AgentWaitEventScan::Outcome);
                }
                if agent.is_some() && expected_agent.is_some() && &agent != expected_agent {
                    return agent_wait_not_running(request_id.to_owned())
                        .map(AgentWaitOutcome::Response)
                        .map(AgentWaitEventScan::Outcome);
                }
                should_probe = true;
            }
            EventData::PaneAgentStatusChanged {
                pane_id: event_pane,
                agent_status,
                ..
            } if event_pane == pane_id => {
                if wait.accept_transient_status && wait.until.contains(&agent_status) {
                    matched_event_status = Some(agent_status);
                }
                should_probe = true;
            }
            EventData::PaneUpdated { pane } if pane.pane_id == pane_id => should_probe = true,
            EventData::PaneClosed {
                pane_id: event_pane,
                ..
            }
            | EventData::PaneExited {
                pane_id: event_pane,
                ..
            } if event_pane == pane_id => {
                return agent_wait_not_running(request_id.to_owned())
                    .map(AgentWaitOutcome::Response)
                    .map(AgentWaitEventScan::Outcome);
            }
            _ => {}
        }
    }
    Ok(AgentWaitEventScan::Probe {
        should_probe,
        matched_event_status,
    })
}

pub(super) fn agent_wait_statuses(
    until: Vec<crate::protocol::api::schema::AgentStatus>,
) -> Vec<crate::protocol::api::schema::AgentStatus> {
    if until.is_empty() {
        vec![
            crate::protocol::api::schema::AgentStatus::Idle,
            crate::protocol::api::schema::AgentStatus::Done,
            crate::protocol::api::schema::AgentStatus::Blocked,
        ]
    } else {
        until
    }
}

pub(super) fn agent_wait_identity_matches(
    agent: &crate::protocol::api::schema::AgentInfo,
    expected_terminal_id: &str,
    expected_name: Option<&str>,
    expected_agent: Option<&str>,
) -> bool {
    agent.terminal_id == expected_terminal_id
        && expected_name.is_none_or(|name| agent.name.as_deref() == Some(name))
        && match (expected_agent, agent.agent.as_deref()) {
            (Some(expected), Some(current)) => expected == current,
            (Some(_), None) => agent.name.is_some(),
            (None, _) => true,
        }
}

pub(super) fn agent_wait_matches(
    agent: &crate::protocol::api::schema::AgentInfo,
    until: &[crate::protocol::api::schema::AgentStatus],
    after_state_change_seq: Option<u64>,
) -> bool {
    until.contains(&agent.agent_status)
        && after_state_change_seq.is_none_or(|baseline| agent.state_change_seq > baseline)
}

fn agent_get(
    request_id: &str,
    target: &str,
    api_tx: &ApiRequestSender,
) -> Result<crate::protocol::api::schema::AgentInfo, ErrorResponse> {
    let response = dispatch_to_app_with_timeout(
        Request {
            id: format!("{request_id}:agent"),
            method: Method::AgentGet(crate::protocol::api::schema::AgentTarget {
                target: target.to_string(),
            }),
        },
        api_tx,
        Some(APP_RESPONSE_TIMEOUT),
    );
    agent_from_response(request_id, &response)
}

pub(super) fn agent_from_response(
    request_id: &str,
    response: &str,
) -> Result<crate::protocol::api::schema::AgentInfo, ErrorResponse> {
    let value: serde_json::Value = serde_json::from_str(response).map_err(|_| ErrorResponse {
        id: request_id.into(),
        error: ErrorBody {
            code: "internal_error".into(),
            message: "failed to decode agent response".into(),
        },
    })?;
    if value.get("error").is_some() {
        let error = serde_json::from_value(value["error"].clone()).map_err(|_| ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "internal_error".into(),
                message: "failed to decode agent error".into(),
            },
        })?;
        return Err(ErrorResponse {
            id: request_id.into(),
            error,
        });
    }
    serde_json::from_value(value["result"]["agent"].clone()).map_err(|_| ErrorResponse {
        id: request_id.into(),
        error: ErrorBody {
            code: "internal_error".into(),
            message: "failed to decode agent result".into(),
        },
    })
}

fn agent_wait_success(
    request_id: String,
    agent: crate::protocol::api::schema::AgentInfo,
) -> std::io::Result<String> {
    serde_json::to_string(&SuccessResponse {
        id: request_id,
        result: ResponseResult::AgentInfo { agent },
    })
    .map_err(std::io::Error::other)
}

fn agent_wait_timeout(
    request_id: String,
    kind: AgentWaitTimeoutKind,
    current: &crate::protocol::api::schema::AgentInfo,
) -> std::io::Result<String> {
    let (code, message) = match kind {
        AgentWaitTimeoutKind::Status => {
            ("timeout", "timed out waiting for agent status".to_string())
        }
        AgentWaitTimeoutKind::PromptStalled { timeout_ms } => {
            let status = format!("{:?}", current.agent_status).to_ascii_lowercase();
            (
                "agent_prompt_stalled",
                format!(
                    "agent prompt produced no observed working or blocked state within {timeout_ms} ms; current status is {status}"
                ),
            )
        }
    };
    serde_json::to_string(&ErrorResponse {
        id: request_id,
        error: ErrorBody {
            code: code.into(),
            message,
        },
    })
    .map_err(std::io::Error::other)
}

pub(super) fn agent_wait_not_running(request_id: String) -> std::io::Result<String> {
    serde_json::to_string(&ErrorResponse {
        id: request_id,
        error: ErrorBody {
            code: "agent_not_running".into(),
            message: "agent is no longer running in the target pane".into(),
        },
    })
    .map_err(std::io::Error::other)
}

fn agent_wait_probe_error(response: ErrorResponse) -> std::io::Result<String> {
    if response.error.code == "agent_not_found" {
        return agent_wait_not_running(response.id);
    }
    serde_json::to_string(&response).map_err(std::io::Error::other)
}

pub(in crate::server::api) fn wait_for_event(
    request_id: String,
    params: EventsWaitParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    let deadline = params
        .timeout_ms
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));

    let subscription = match event_match_subscription(&request_id, params.match_event) {
        Ok(subscription) => subscription,
        Err(response) => {
            return Ok(Some(
                serde_json::to_string(&response).map_err(std::io::Error::other)?,
            ))
        }
    };
    let mut active = match ActiveSubscription::new(
        subscription,
        &request_id,
        0,
        api_tx,
        event_hub,
        event_hub.current_sequence(),
    ) {
        Ok(active) => active,
        Err(response) => {
            return Ok(Some(
                serde_json::to_string(&response).map_err(std::io::Error::other)?,
            ))
        }
    };

    loop {
        if should_stop_connection(stream, running)? {
            return Ok(None);
        }

        match active.poll_for_wait(api_tx, event_hub) {
            Ok(Some(event)) => return Ok(Some(wait_matched_response(&request_id, event)?)),
            Ok(None) => {}
            Err(mut response) if response.error.code == "pane_not_found" => {
                response.id = request_id;
                return serde_json::to_string(&response)
                    .map(Some)
                    .map_err(std::io::Error::other);
            }
            Err(_) => {}
        }

        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            return Ok(Some(
                serde_json::to_string(&ErrorResponse {
                    id: request_id,
                    error: ErrorBody {
                        code: "timeout".into(),
                        message: "timed out waiting for event match".into(),
                    },
                })
                .map_err(std::io::Error::other)?,
            ));
        }

        std::thread::sleep(CONNECTION_POLL_INTERVAL);
    }
}

fn event_match_subscription(
    request_id: &str,
    match_event: EventMatch,
) -> Result<Subscription, ErrorResponse> {
    match match_event {
        EventMatch::PaneAgentStatusChanged {
            pane_id,
            agent_status,
        } => Ok(Subscription::PaneAgentStatusChanged {
            pane_id,
            agent_status: Some(agent_status),
        }),
        _ => Err(ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "unsupported_event_wait_match".into(),
                message: "events.wait currently supports pane agent status matches".into(),
            },
        }),
    }
}

fn wait_matched_response(request_id: &str, event: serde_json::Value) -> std::io::Result<String> {
    let Ok(event) = serde_json::from_value::<SubscriptionEventEnvelope>(event) else {
        return serde_json::to_string(&ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "internal_error".into(),
                message: "failed to decode matched event".into(),
            },
        })
        .map_err(std::io::Error::other);
    };

    let SubscriptionEventData::PaneAgentStatusChanged(data) = event.data else {
        return serde_json::to_string(&ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "unsupported_event_wait_match".into(),
                message: "events.wait currently supports pane agent status matches".into(),
            },
        })
        .map_err(std::io::Error::other);
    };

    serde_json::to_string(&SuccessResponse {
        id: request_id.into(),
        result: ResponseResult::WaitMatched {
            event: EventEnvelope {
                event: EventKind::PaneAgentStatusChanged,
                data: EventData::PaneAgentStatusChanged {
                    pane_id: data.pane_id,
                    workspace_id: data.workspace_id,
                    agent_status: data.agent_status,
                    agent: data.agent,
                    title: data.title,
                    display_agent: data.display_agent,
                    state_labels: data.state_labels,
                },
            },
        },
    })
    .map_err(std::io::Error::other)
}

#[cfg(test)]
#[path = "tests/wait_test.rs"]
mod tests;
