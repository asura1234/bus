use super::{
    callbacks, AgentId, BusState, CallbackEventKind, CallbackRecordResult, ControlFlow, Path,
    ProviderCallback, ProviderKind, Worker,
};

pub(super) fn started_callback(
    record: &callbacks::Record,
    id: AgentId,
    session: String,
    turn: String,
    prompt: String,
) -> ProviderCallback {
    super::callback(
        record,
        id,
        session,
        turn,
        Some(prompt),
        CallbackEventKind::PromptStarted,
    )
}

impl Worker {
    pub(super) fn completion_callback(
        &self,
        id: AgentId,
        state: BusState,
        path: &Path,
        record: &callbacks::Record,
        session: String,
        candidate_turn: Option<String>,
    ) -> Result<CallbackRecordResult, String> {
        if record.manifest.provider == ProviderKind::Kimi {
            return Ok(ControlFlow::Continue((
                state,
                None,
                vec![path.to_path_buf()],
            )));
        }
        self.grok_completion_callback(id, state, path, record, session, candidate_turn)
    }
}
