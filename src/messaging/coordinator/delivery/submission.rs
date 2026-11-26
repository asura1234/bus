//! Native submission method construction and response timestamps.
use super::super::{
    diagnostics, io, launch, schema, Method, RequestId, ResponseResult, RoomAgent, Worker,
};
use crate::messaging::model::DispatchMark;
use crate::messaging::native::TransportError;

pub(in crate::messaging::coordinator) fn native_response_timestamps(
    response: &Result<ResponseResult, TransportError>,
) -> crate::protocol::api::schema::NativeSubmissionTimestamps {
    match response {
        Ok(ResponseResult::AgentPrompted { timestamps, .. }) => *timestamps,
        Err(error) => *error.timestamps,
        _ => Default::default(),
    }
}

pub(in crate::messaging::coordinator) fn record_native_timestamps(
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
    allow_working: bool,
    generation: u64,
    discard_aborted_payload: Option<schema::AbortedPayloadAction>,
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
            allow_working,
            expected_generation: Some(generation),
            expected_turn_id: None,
            discard_aborted_payload,
        })
    } else {
        Method::AgentPromptIfUnboundAtGeneration(schema::AgentPromptIfUnboundParams {
            target: pane.to_owned(),
            text,
            expected_terminal_id: terminal.to_owned(),
            expected_pane_id: pane.to_owned(),
            expected_managed_name: format!("bus-r{}-a{}", agent.room_id.0, agent.id.0),
            allow_working,
            expected_generation: Some(generation),
        })
    }
}

impl Worker {
    /// Types `request` into the agent's running turn, the way a person types
    /// while an agent works, so it joins `lead`'s group and shares its reply.
    pub(in crate::messaging::coordinator) fn steer(
        &mut self,
        agent: &RoomAgent,
        request: RequestId,
        lead: RequestId,
    ) -> Result<(), String> {
        let identity = &agent.runtime_identity;
        let (Some(terminal), Some(pane), Some(_session)) = (
            &identity.terminal_id,
            &identity.pane_id,
            &identity.session_id,
        ) else {
            return Ok(());
        };
        let mut state = self.state.clone();
        self.mark_open_dispatch(&mut state, request, DispatchMark::Started);
        let text = state
            .render_request_payload(request)
            .ok_or("Missing queued request")?;
        self.mark_open_dispatch(&mut state, request, DispatchMark::Render);
        state
            .begin_steering(request, lead)
            .map_err(|e| e.to_string())?;
        self.stamp_steering_origin(&mut state, agent, request, terminal, pane)?;
        state
            .record_picked(request, io::now_ms(), true)
            .map_err(|e| e.to_string())?;
        state.begin_dialog_notice_write(request, io::now_ms());
        self.mark_open_dispatch(&mut state, request, DispatchMark::Notice);
        state
            .record_native_requested(request, io::now_ms())
            .map_err(|error| error.to_string())?;
        self.save(state)?;
        self.mark_live_dispatch(request, DispatchMark::Save);
        diagnostics::request(&self.state, request, "bus.delivery.start", "steer");
        self.mark_live_dispatch(request, DispatchMark::Diagnostics);
        let started = std::time::Instant::now();
        let mut method = submission_method(
            agent,
            text,
            terminal,
            pane,
            true,
            false,
            self.serving_generation,
            None,
        );
        if let Method::AgentPromptIfIdleAtGeneration(params) = &mut method {
            params.expected_turn_id = self
                .state
                .request(lead)
                .and_then(|request| request.provider_turn_id.clone());
        }
        self.mark_live_dispatch(request, DispatchMark::Method);
        self.start_native_submission(agent, request, method, true, started)
    }
}
