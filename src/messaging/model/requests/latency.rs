use super::{BusState, ModelError, RequestId};

impl BusState {
    pub(crate) fn record_queued(
        &mut self,
        request: RequestId,
        at_ms: u64,
    ) -> Result<(), ModelError> {
        let request = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        request.latency.queued_at_ms.get_or_insert(at_ms);
        Ok(())
    }

    fn delivery_targets(&self, request: RequestId) -> Result<Vec<RequestId>, ModelError> {
        self.requests
            .get(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        let mut targets = vec![request];
        targets.extend(self.group_members(request).into_iter().filter(|id| {
            *id != request
                && self
                    .requests
                    .get(id)
                    .is_some_and(|request| !request.steered)
        }));
        Ok(targets)
    }

    pub(crate) fn record_picked(
        &mut self,
        request: RequestId,
        at_ms: u64,
        agent_waited: bool,
    ) -> Result<(), ModelError> {
        for id in self.delivery_targets(request)? {
            if let Some(request) = self.requests.get_mut(&id) {
                request.latency.picked_at_ms.get_or_insert(at_ms);
                request.latency.agent_waited |= agent_waited;
            }
        }
        Ok(())
    }

    pub(crate) fn record_native_requested(
        &mut self,
        request: RequestId,
        at_ms: u64,
    ) -> Result<(), ModelError> {
        for id in self.delivery_targets(request)? {
            if let Some(request) = self.requests.get_mut(&id) {
                request.latency.native_requested_at_ms.get_or_insert(at_ms);
            }
        }
        Ok(())
    }

    pub(crate) fn record_native_timestamps(
        &mut self,
        request: RequestId,
        timestamps: crate::protocol::api::schema::NativeSubmissionTimestamps,
    ) -> Result<(), ModelError> {
        for id in self.delivery_targets(request)? {
            if let Some(request) = self.requests.get_mut(&id) {
                request.latency.native.merge_from(timestamps);
            }
        }
        Ok(())
    }

    pub(crate) fn record_provider_started(&mut self, request: RequestId, at_ms: u64) {
        if let Ok(targets) = self.delivery_targets(request) {
            for id in targets {
                if let Some(request) = self.requests.get_mut(&id) {
                    request.latency.provider_started_at_ms.get_or_insert(at_ms);
                }
            }
        }
    }

    pub(crate) fn record_reply_captured(&mut self, request: RequestId, at_ms: u64) {
        if let Ok(targets) = self.delivery_targets(request) {
            for id in targets {
                if let Some(request) = self.requests.get_mut(&id) {
                    request.latency.reply_captured_at_ms.get_or_insert(at_ms);
                }
            }
        }
    }
}
