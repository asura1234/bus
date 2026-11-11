use super::{BusState, CallbackDisposition, CallbackEventKind, ProviderCallback};

impl BusState {
    pub(crate) fn accept_callback(
        &mut self,
        mut callback: ProviderCallback,
    ) -> CallbackDisposition {
        self.match_owned_claude_paste(&mut callback);
        let agent = callback.agent_id;
        let occurred_at_ms = callback.occurred_at_ms;
        let before = self.agents.get(&agent).and_then(|a| a.current_request);
        let previous_turn = before
            .and_then(|id| self.requests.get(&id))
            .map(|request| (activity_key(request), request.bound_turn_activity.clone()));
        let final_event =
            matches!(callback.kind, CallbackEventKind::Final { .. }).then(|| callback.clone());
        let reply_event = final_event.is_some();
        let started_event = matches!(callback.kind, CallbackEventKind::PromptStarted);
        let disposition = self.accept_callback_unrecorded(callback);
        // Retry acceptance rebinds current_request onto the steered member.
        // A completed final clears it, so that path keeps the request that
        // was current when the callback arrived.
        let current = self
            .agents
            .get(&agent)
            .and_then(|agent_state| agent_state.current_request)
            .or(before);
        if let Some(callback) = final_event {
            self.record_worker_final(&callback, &disposition);
        }
        if !matches!(disposition, CallbackDisposition::Rejected(_)) {
            if let Some(request) = current {
                if started_event {
                    self.record_provider_started(request, occurred_at_ms);
                    if let Some(bound) = self.requests.get_mut(&request) {
                        let activity = previous_turn
                            .as_ref()
                            .filter(|(key, _)| *key == activity_key(bound))
                            .and_then(|(_, activity)| activity.clone());
                        let mut activity =
                            activity.unwrap_or(crate::messaging::model::types::BoundTurnActivity {
                                started_at_ms: occurred_at_ms,
                                idle_since_ms: None,
                                working_seen: false,
                            });
                        // New accepted input is progress even if the pane
                        // repeats Idle. Keep the Working latch, renew grace.
                        activity.started_at_ms = activity.started_at_ms.max(occurred_at_ms);
                        activity.idle_since_ms = None;
                        bound.bound_turn_activity = Some(activity);
                    }
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
        // An accepted provider event records ordering evidence for this request.
        // It does not prove a subsequent Idle poll contains a fresh frame:
        // unchanged pre-Started Idle cannot establish a bound turn's end.
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

// Prompt IDs can change when input joins an existing native turn. Preserve
// its Working evidence; only a distinct native turn clears that latch.
fn activity_key(request: &super::Request) -> (Option<String>, Option<String>, Option<String>) {
    (
        request.provider_session_id.clone(),
        request.provider_turn_id.clone(),
        if request.provider_turn_id.is_none() {
            request.provider_prompt_id.clone()
        } else {
            None
        },
    )
}
