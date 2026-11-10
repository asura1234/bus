use super::super::DispatchMark;
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

    pub(crate) fn finish_queued(
        &mut self,
        request: RequestId,
        started_at_ms: u64,
        completed_at_ms: u64,
    ) -> Result<(), ModelError> {
        let request = self
            .requests
            .get_mut(&request)
            .ok_or(ModelError::UnknownRequest(request))?;
        if request.latency.queued_at_ms == Some(started_at_ms) && completed_at_ms >= started_at_ms {
            request.latency.queued_at_ms = Some(completed_at_ms);
        }
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

    pub(crate) fn record_eligible(&mut self, request: RequestId, at_ms: u64) {
        if let Ok(targets) = self.delivery_targets(request) {
            for id in targets {
                if let Some(request) = self.requests.get_mut(&id) {
                    request.latency.eligible_at_ms.get_or_insert(at_ms);
                }
            }
        }
    }

    pub(crate) fn record_picked(
        &mut self,
        request: RequestId,
        at_ms: u64,
        agent_waited: bool,
    ) -> Result<(), ModelError> {
        let marks = self
            .requests
            .get(&request)
            .map(|request| request.latency.dispatch)
            .unwrap_or_default();
        for id in self.delivery_targets(request)? {
            if let Some(request) = self.requests.get_mut(&id) {
                request.latency.picked_at_ms.get_or_insert(at_ms);
                request.latency.agent_waited |= agent_waited;
                if request.latency.dispatch.started_at_ms.is_none() {
                    request.latency.dispatch = marks;
                }
            }
        }
        Ok(())
    }

    /// Records one dispatch step on the lead and its current group.
    ///
    /// Overwrites a previous attempt. Does not invent `Some(0)` when the
    /// step has not run.
    pub(crate) fn record_dispatch_mark(
        &mut self,
        request: RequestId,
        mark: DispatchMark,
        at_ms: u64,
    ) {
        let Ok(targets) = self.delivery_targets(request) else {
            return;
        };
        for id in targets {
            let Some(request) = self.requests.get_mut(&id) else {
                continue;
            };
            let slot = match mark {
                DispatchMark::Started => &mut request.latency.dispatch.started_at_ms,
                DispatchMark::Render => &mut request.latency.dispatch.render_at_ms,
                DispatchMark::Boundary => &mut request.latency.dispatch.boundary_at_ms,
                DispatchMark::Notice => &mut request.latency.dispatch.notice_at_ms,
                DispatchMark::Save => &mut request.latency.dispatch.save_at_ms,
                DispatchMark::Diagnostics => &mut request.latency.dispatch.diagnostics_at_ms,
                DispatchMark::Method => &mut request.latency.dispatch.method_at_ms,
            };
            *slot = Some(at_ms);
        }
    }

    pub(crate) fn record_native_requested(
        &mut self,
        request: RequestId,
        at_ms: u64,
    ) -> Result<(), ModelError> {
        for id in self.delivery_targets(request)? {
            if let Some(request) = self.requests.get_mut(&id) {
                request.latency.native_requested_at_ms = Some(at_ms);
            }
        }
        Ok(())
    }

    pub(crate) fn finish_native_requested(
        &mut self,
        request: RequestId,
        started_at_ms: u64,
        completed_at_ms: u64,
    ) -> Result<(), ModelError> {
        for id in self.delivery_targets(request)? {
            if let Some(request) = self.requests.get_mut(&id) {
                if request.latency.native_requested_at_ms == Some(started_at_ms)
                    && completed_at_ms >= started_at_ms
                {
                    request.latency.native_requested_at_ms = Some(completed_at_ms);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn record_native_timestamps(
        &mut self,
        request: RequestId,
        timestamps: crate::messaging::model::NativeSubmissionTimestamps,
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

    pub(crate) fn record_reply_delivered(&mut self, request: RequestId, at_ms: u64) {
        if let Some(request) = self.requests.get_mut(&request) {
            request.latency.reply_delivered_at_ms.get_or_insert(at_ms);
        }
    }
}
