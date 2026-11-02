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
    pub(super) fn started_request_callback(
        &mut self,
        id: AgentId,
        state: &mut BusState,
        record: &callbacks::Record,
        session: String,
        turn: String,
        prompt: String,
    ) -> Result<ProviderCallback, String> {
        let started = started_callback(record, id, session.clone(), turn.clone(), prompt);
        if record.manifest.provider == ProviderKind::Kimi
            && state
                .agent(id)
                .and_then(|agent| agent.current_request)
                .and_then(|request| state.request(request))
                .is_some_and(|request| request.provider_turn_id.as_deref() != Some(&turn))
        {
            // The wire is authoritative even when Stop was lost or reordered.
            if let Some(callback) =
                self.committed_kimi_completion_callback(id, state, record, session, Some(&started))?
            {
                self.apply_request_callback(id, state, callback)?;
            }
        }
        Ok(started)
    }

    fn committed_kimi_completion_callback(
        &self,
        id: AgentId,
        state: &mut BusState,
        record: &callbacks::Record,
        session: String,
        started: Option<&ProviderCallback>,
    ) -> Result<Option<ProviderCallback>, String> {
        Ok(match self.recorded_kimi_end(state, id)? {
            Some(crate::messaging::model::RecordedTurnEnd::Final {
                turn_id,
                text,
                prompts,
            }) => {
                state.bind_recorded_turn_inputs(id, &turn_id, &prompts);
                let mut callback = super::callback(
                    record,
                    id,
                    session,
                    turn_id.clone(),
                    None,
                    CallbackEventKind::Final { text },
                );
                callback.callback_id = format!("{}:kimi-wire:{turn_id}", record.id);
                Some(callback)
            }
            Some(crate::messaging::model::RecordedTurnEnd::Error(error)) => {
                // Stop has no next-turn identity. Periodic settlement runs
                // after this batch, allowing a later Started to claim its
                // submitted input before the failed group closes.
                if let Some(started) = started {
                    state.settle_kimi_failure_before_start(started, error);
                }
                None
            }
            _ => None,
        })
    }

    pub(super) fn completion_callback(
        &self,
        id: AgentId,
        mut state: BusState,
        path: &Path,
        record: &callbacks::Record,
        session: String,
        candidate_turn: Option<String>,
    ) -> Result<CallbackRecordResult, String> {
        if record.manifest.provider == ProviderKind::Kimi {
            // Stop carries no turn id. Read the committed owned turn before
            // processing a queued prompt's Started callback, which otherwise
            // rebinds the old lead and makes its final impossible to match.
            let callback =
                self.committed_kimi_completion_callback(id, &mut state, record, session, None)?;
            return Ok(ControlFlow::Continue((
                state,
                callback,
                vec![path.to_path_buf()],
            )));
        }
        self.grok_completion_callback(id, state, path, record, session, candidate_turn)
    }
}
