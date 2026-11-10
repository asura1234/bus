//! Native submission method construction and response timestamps.
use super::super::{launch, schema, Method, RequestId, ResponseResult, RoomAgent};
use crate::messaging::native::TransportError;

pub(super) fn native_response_timestamps(
    response: &Result<ResponseResult, TransportError>,
) -> crate::protocol::api::schema::NativeSubmissionTimestamps {
    match response {
        Ok(ResponseResult::AgentPrompted { timestamps, .. }) => *timestamps,
        Err(error) => *error.timestamps,
        _ => Default::default(),
    }
}

pub(super) fn record_native_timestamps(
    state: &mut crate::messaging::model::BusState,
    request: RequestId,
    timestamps: crate::protocol::api::schema::NativeSubmissionTimestamps,
) -> Result<(), String> {
    state
        .record_native_timestamps(request, timestamps.into())
        .map_err(|error| error.to_string())
}

pub(super) fn submission_method(
    agent: &RoomAgent,
    text: String,
    terminal: &str,
    pane: &str,
    steer: bool,
    generation: u64,
) -> Method {
    if let Some(session) = &agent.runtime_identity.session_id {
        Method::AgentPromptIfIdleAtGeneration(schema::AgentPromptIfIdleParams {
            target: pane.to_owned(),
            text,
            expected_terminal_id: terminal.to_owned(),
            expected_pane_id: pane.to_owned(),
            expected_agent: launch::provider_kind(agent.provider).into(),
            expected_session_id: session.clone(),
            steer,
            expected_generation: Some(generation),
            expected_turn_id: None,
        })
    } else {
        Method::AgentPromptIfUnboundAtGeneration(schema::AgentPromptIfUnboundParams {
            target: pane.to_owned(),
            text,
            expected_terminal_id: terminal.to_owned(),
            expected_pane_id: pane.to_owned(),
            expected_managed_name: format!("bus-r{}-a{}", agent.room_id.0, agent.id.0),
            expected_generation: Some(generation),
        })
    }
}
