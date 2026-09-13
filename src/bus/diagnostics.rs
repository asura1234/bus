//! Content-free delivery diagnostics. Never format commands, prompts or callbacks with Debug.
use super::model::*;

// Upstream input/toast diagnostics contain user content. Keep those payload dumps
// disabled even in dev mode; the rest of the runtime retains TRACE visibility.
pub(crate) const DEV_FILTER: &str =
    "herdr=trace,herdr::raw_input=off,herdr::client::input=info,herdr::private_payload=off";
pub(crate) const EXISTING_SERVER_NOTICE: &str =
    "Dev logs enabled for this client. An existing server keeps its original log level; restart it when safe for full server logs. Agents were not restarted.";

pub(crate) fn dev_enabled() -> bool {
    std::env::var_os("BUS_DEV").is_some_and(|value| value == "1")
}

pub(crate) fn request(state: &BusState, id: RequestId, event: &'static str, reason: &str) {
    let Some(request) = state.request(id) else {
        return;
    };
    let agent = state.agent(request.agent_id);
    tracing::info!(
        event, reason, request_id = id.0, prompt_id = request.prompt.id.0,
        room_id = request.room_id.0, agent_id = request.agent_id.0,
        provider = ?agent.map(|a| a.provider), phase = ?request.phase,
        launch_id = ?agent.and_then(|a| a.runtime_identity.launch_id.as_deref()),
        terminal_id = ?agent.and_then(|a| a.runtime_identity.terminal_id.as_deref()),
        pane_id = ?agent.and_then(|a| a.runtime_identity.pane_id.as_deref()),
        session_id = ?request.provider_session_id, turn_id = ?request.provider_turn_id,
        boundary = ?request.submission_boundary,
        elapsed_ms = super::io::now_ms().saturating_sub(request.prompt.submitted_at_ms),
        "Bus delivery"
    );
}

pub(crate) fn wait_reason(agent: &Agent) -> Option<&'static str> {
    let identity = &agent.runtime_identity;
    if agent.deletion_pending {
        Some("deletion_pending")
    } else if agent.session_binding_invalidated {
        Some("session_invalidated")
    } else if !agent.hook_setup_confirmed {
        Some("hook_setup_unconfirmed")
    } else if agent.status != RuntimeStatus::Idle {
        Some(match agent.status {
            RuntimeStatus::Blocked => "agent_blocked",
            RuntimeStatus::Working => "agent_working",
            RuntimeStatus::Launching => "agent_launching",
            _ => "agent_unavailable",
        })
    } else if agent.current_request.is_some() {
        Some("prior_request_active")
    } else if identity.launch_id.is_none()
        || identity.terminal_id.is_none()
        || identity.pane_id.is_none()
    {
        Some("terminal_identity_missing")
    } else if identity.session_id.is_none() && agent.provider != Provider::Codex {
        Some("session_hook_missing")
    } else {
        None
    }
}

/// Called after durable save, and separately when a new snapshot reaches the UI.
/// Bounded by rooms × agents, not total historical requests. No render-time work.
pub(crate) fn replies(previous: &BusState, next: &BusState, event: &'static str) {
    for room in next.rooms() {
        for (agent, reply) in &room.latest_replies {
            if previous
                .room(room.id)
                .and_then(|r| r.latest_replies.get(agent))
                .is_none_or(|old| old.request_id != reply.request_id)
            {
                request(next, reply.request_id, event, "completed");
                tracing::debug!(
                    event = "bus.reply.size",
                    request_id = reply.request_id.0,
                    reply_bytes = reply.text.len(),
                    "Bus reply metadata"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dev_logs_enable_debug_and_trace_but_not_input_or_toast_payloads() {
        let capture = crate::logging::test_capture::Capture::default();
        capture.run_filtered(super::DEV_FILTER, || {
            tracing::debug!(target: "herdr::bus", "DEBUG_ENABLED");
            tracing::trace!(target: "herdr::server::headless", "TRACE_ENABLED");
            tracing::debug!(target: "herdr::raw_input", "SECRET_TYPED_TEXT");
            tracing::debug!(target: "herdr::client::input", "SECRET_PASTED_TEXT");
            tracing::debug!(target: "herdr::private_payload", "SECRET_TOAST");
            tracing::info!(target: "herdr::bus", "NORMAL_EVENTS");
        });
        let logs = capture.text();
        assert!(
            logs.contains("DEBUG_ENABLED") && logs.contains("TRACE_ENABLED"),
            "{logs}"
        );
        assert!(logs.contains("NORMAL_EVENTS"), "{logs}");
        assert!(!logs.contains("SECRET"), "{logs}");
    }
}
