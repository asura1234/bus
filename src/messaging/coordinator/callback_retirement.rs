//! Read callback evidence before releasing an interrupted Claude input group.
use super::{
    callback, callbacks, order_records, AgentId, BusState, CallbackEventKind, Parsed,
    ProviderCallback, RuntimeStatus, Worker,
};

impl Worker {
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
