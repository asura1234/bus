use super::{BusState, CallbackDisposition, CallbackEventKind, ProviderCallback};

impl BusState {
    pub(crate) fn accept_callback(
        &mut self,
        mut callback: ProviderCallback,
    ) -> CallbackDisposition {
        self.match_owned_claude_paste(&mut callback);
        let agent = callback.agent_id;
        let occurred_at_ms = callback.occurred_at_ms;
        let current = self.agents.get(&agent).and_then(|a| a.current_request);
        let final_event =
            matches!(callback.kind, CallbackEventKind::Final { .. }).then(|| callback.clone());
        let reply_event = final_event.is_some();
        let started_event = matches!(callback.kind, CallbackEventKind::PromptStarted);
        let disposition = self.accept_callback_unrecorded(callback);
        if let Some(callback) = final_event {
            self.record_worker_final(&callback, &disposition);
        }
        if !matches!(disposition, CallbackDisposition::Rejected(_)) {
            if let Some(request) = current {
                if started_event {
                    self.record_provider_started(request, occurred_at_ms);
                }
                if reply_event {
                    self.record_reply_captured(request, occurred_at_ms);
                }
            }
            if let Some(request) = current.and_then(|id| self.requests.get_mut(&id)) {
                let progress = request.progress_at_ms.get_or_insert(occurred_at_ms);
                *progress = (*progress).max(occurred_at_ms);
            }
        }
        // A provider turn event on a Bus request proves the agent was busy
        // with it, even when the turn started and ended between two status
        // polls that only ever saw Idle. Recording the edge here lets
        // `turn_ended` (and so `send --async`) finish on the next Idle poll.
        if matches!(
            disposition,
            CallbackDisposition::AcceptedBinding
                | CallbackDisposition::AcceptedContinuation
                | CallbackDisposition::AcceptedSteering
                | CallbackDisposition::AcceptedProgress
                | CallbackDisposition::AcceptedPendingSettlement
        ) {
            self.record_busy_edge(agent);
        }
        disposition
    }
}
