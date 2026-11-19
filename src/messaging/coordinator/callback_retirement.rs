//! Read callback evidence before releasing an interrupted Claude input group.
use super::{
    callback, callbacks, order_records, AgentId, BusState, CallbackEventKind, Method, Parsed,
    ProviderCallback, ResponseResult, RuntimeStatus, Worker,
};

impl Worker {
    /// Verify the original terminal with a read-only native lookup. A missing
    /// pane or a different terminal at the same address can leave the original
    /// provider running in a detached terminal.
    pub(in crate::messaging::coordinator) fn resolve_exited_claude_input(
        &mut self,
        state: &mut BusState,
        agent: AgentId,
        now_ms: u64,
    ) -> Result<bool, String> {
        let Some(request) = state.interrupt_retirement_owner(agent) else {
            return Ok(false);
        };
        let Some(original) = state.interrupt_retirement_origin(request).cloned() else {
            return Ok(false);
        };
        let (Some(pane), Some(terminal)) = (&original.pane_id, &original.terminal_id) else {
            return Ok(false);
        };
        let observation = self
            .transport
            .request(Method::PaneGet(super::schema::PaneTarget {
                pane_id: pane.clone(),
            }));
        let original_exited = matches!(&observation, Ok(ResponseResult::PaneInfo { pane: observed })
            if observed.pane_id == *pane && observed.terminal_id == *terminal
                && observed.agent_process_exited
                && original.session_id.as_ref().is_some_and(|session| {
                    observed.agent_exited_session_id.as_ref() == Some(session)
                })
                && observed.agent.is_none()
                && observed.agent_session.is_none());
        let original_absent = !original_exited
            && !matches!(observation, Ok(ResponseResult::PaneInfo { pane: observed })
                if observed.terminal_id == *terminal)
            && matches!(
                self.transport.request(Method::TerminalRequireAbsent(
                    super::schema::TerminalTarget {
                        terminal_id: terminal.clone()
                    },
                )),
                Ok(ResponseResult::TerminalAbsent { terminal_id }) if terminal_id == *terminal
            );
        if !original_exited && !original_absent {
            return Ok(false);
        }
        let callbacks = self.retirement_callback_evidence(state, agent)?;
        state.reconcile_retirement_answers(agent, &callbacks);
        state
            .resolve_exited_interrupt_terminal(request, &original, now_ms)
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    pub(in crate::messaging::coordinator) fn reconcile_polled_claude_retirement(
        &self,
        state: &mut BusState,
        id: AgentId,
        status: RuntimeStatus,
    ) -> Result<(), String> {
        if matches!(
            status,
            RuntimeStatus::Idle | RuntimeStatus::Unavailable | RuntimeStatus::Launching
        ) && state.agent(id).is_some_and(|agent| agent.interrupt_pending)
            && state.interrupt_retirement_evidence_owner(id).is_some()
        {
            // Hooks may publish between the native reply and the settling poll.
            let evidence = self.retirement_callback_evidence(state, id)?;
            state.reconcile_retirement_answers(id, &evidence);
        }
        Ok(())
    }

    pub(in crate::messaging::coordinator) fn retirement_callback_evidence(
        &self,
        state: &BusState,
        id: AgentId,
    ) -> Result<Vec<ProviderCallback>, String> {
        let owner = state.agent(id).ok_or("Unknown callback agent")?;
        let Some(identity) = state
            .interrupt_retirement_evidence_owner(id)
            .and_then(|lead| state.request(lead))
            .and_then(|lead| lead.native_delivery_identity.as_ref())
        else {
            return Ok(Vec::new());
        };
        let (Some(launch), Some(original_session)) = (&identity.launch_id, &identity.session_id)
        else {
            return Ok(Vec::new());
        };
        let mut records = callbacks::records(&self.data_dir.join("callbacks").join(launch))
            .map_err(|error| {
                format!("Cannot reconcile Claude callbacks before retirement: {error}")
            })?;
        order_records(&mut records);
        Ok(records
            .iter()
            .filter_map(|(_, record)| {
                if record.nested_provider
                    || record.manifest.routing_key != callbacks::RoutingKey(id.0)
                    || record.manifest.provider != owner.provider
                    || &record.manifest.launch_id != launch
                {
                    return None;
                }
                let (session, turn, prompt, kind) =
                    match callbacks::parse(owner.provider, &record.value).ok()? {
                        Parsed::Started {
                            session,
                            turn,
                            prompt,
                        } => (
                            session,
                            turn,
                            Some(prompt),
                            CallbackEventKind::PromptStarted,
                        ),
                        Parsed::Final {
                            session,
                            turn,
                            text,
                        } if !text.trim().is_empty() => {
                            (session, turn, None, CallbackEventKind::Final { text })
                        }
                        _ => return None,
                    };
                (&session == original_session)
                    .then(|| callback(record, id, session, turn, prompt, kind))
            })
            .collect())
    }
}
