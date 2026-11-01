//! Content-free delivery diagnostics. Never format commands, prompts or callbacks with Debug.
use {
    super::model::{BusState, RequestId},
    crate::messaging::model::RoomAgent,
};

// Keep the existing test namespace while startup composition consumes the leaf.
pub(crate) fn wait_reason(agent: &RoomAgent) -> Option<&'static str> {
    agent.wait_reason()
}

#[cfg(test)]
pub(crate) use crate::utils::logging::DEV_FILTER;
pub(crate) const EXISTING_SERVER_NOTICE: &str =
    "Dev logs enabled for this client. An existing server keeps its original log level; restart it when safe for full server logs. Agents were not restarted.";

pub(crate) fn dev_enabled() -> bool {
    std::env::var_os("BUS_DEV").is_some_and(|value| value == "1")
}

pub(crate) fn request(state: &BusState, id: RequestId, event: &'static str, reason: &str) {
    log_request(state, id, event, reason, None);
}

/// Same fields as [`request`], plus the settlement lead. After transcript
/// recovery detaches a follower, `request.group` is `None`, so the lead id
/// cannot be recovered from the request. It must be an event field: a
/// `bus.settlement` span is a different callsite and can be missing from the
/// formatted line while this INFO event is still recorded.
pub(crate) fn request_with_lead(
    state: &BusState,
    id: RequestId,
    event: &'static str,
    reason: &str,
    lead_id: u64,
) {
    log_request(state, id, event, reason, Some(lead_id));
}

fn log_request(
    state: &BusState,
    id: RequestId,
    event: &'static str,
    reason: &str,
    lead_id: Option<u64>,
) {
    let Some(request) = state.request(id) else {
        return;
    };
    let agent = state.agent(request.agent_id);
    let prompt_id = request.prompt.id.0;
    let room_id = request.room_id.0;
    let agent_id = request.agent_id.0;
    let provider = agent.map(|a| a.provider);
    let phase = request.phase;
    let launch_id = agent.and_then(|a| a.runtime_identity.launch_id.as_deref());
    let terminal_id = agent.and_then(|a| a.runtime_identity.terminal_id.as_deref());
    let pane_id = agent.and_then(|a| a.runtime_identity.pane_id.as_deref());
    let session_id = request.provider_session_id.as_deref();
    let turn_id = request.provider_turn_id.as_deref();
    let boundary = request.submission_boundary;
    let completed_at_ms = request.completed_at_ms;
    let status = agent.map(|a| a.status);
    let group = request.group;
    let start_bound = request.trusted_start_bound;
    let steered = request.steered;
    let uncertain_outcome = request.uncertain_outcome;
    let turn_ended_at_ms = request.turn_ended_at_ms;
    let progress_at_ms = request.progress_at_ms;
    let pending_final_bytes = request
        .pending_final
        .as_ref()
        .map(|answer| answer.text.len());
    let elapsed_ms = super::storage::io::now_ms().saturating_sub(request.prompt.submitted_at_ms);
    // One callsite: `Option<u64>` records the integer, or `field::Empty` when
    // absent. A second `tracing::info!` would be a new site that a sibling
    // without a subscriber can cache as `Interest::never`.
    tracing::info!(
        event, reason, request_id = id.0, prompt_id, room_id, agent_id,
        provider = ?provider, phase = ?phase, launch_id = ?launch_id,
        terminal_id = ?terminal_id, pane_id = ?pane_id, session_id = ?session_id,
        turn_id = ?turn_id, boundary = ?boundary, completed_at_ms = ?completed_at_ms,
        status = ?status, group = ?group, start_bound, steered, uncertain_outcome,
        turn_ended_at_ms = ?turn_ended_at_ms, progress_at_ms = ?progress_at_ms,
        pending_final_bytes = ?pending_final_bytes, elapsed_ms, lead_id,
        "Bus delivery"
    );
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
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run_filtered(super::DEV_FILTER, || {
            tracing::debug!(target: "bus::messaging", "DEBUG_ENABLED");
            tracing::trace!(target: "bus::server::main_loop", "TRACE_ENABLED");
            tracing::debug!(target: "bus::protocol::keys::host", "SECRET_TYPED_TEXT");
            tracing::debug!(target: "bus::client::host_terminal::input", "SECRET_PASTED_TEXT");
            tracing::debug!(target: "bus::private_payload", "SECRET_TOAST");
            tracing::info!(target: "bus::messaging", "NORMAL_EVENTS");
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
