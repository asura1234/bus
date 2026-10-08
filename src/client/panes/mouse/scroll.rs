use super::{
    ClientShellEndpointError, ClientShellInput, ClientShellState, PaneHit, PendingEndpointKind,
};

impl ClientShellState {
    pub(super) fn pane_scrollbar_offset(
        hit: &PaneHit,
        row: u16,
        grab_row_offset: Option<u16>,
    ) -> Option<usize> {
        let track = hit.scrollbar_rect?;
        let metrics = hit.scroll?;
        (metrics.max_offset_from_bottom > 0).then(|| match grab_row_offset {
            Some(grab_row_offset) => {
                crate::ui::scrollbar_offset_from_drag_row(metrics, track, row, grab_row_offset)
            }
            None => crate::ui::scrollbar_offset_from_row(metrics, track, row),
        })
    }

    pub(in crate::client) fn push_pane_scroll_offset(
        &mut self,
        pane_id: String,
        offset_from_bottom: usize,
        outcome: &mut ClientShellInput,
    ) {
        self.pane_scroll_targets
            .insert(pane_id.clone(), offset_from_bottom);
        if self.pane_scroll_in_flight.contains_key(&pane_id) {
            self.pane_scroll_queued.insert(pane_id, offset_from_bottom);
            return;
        }
        self.dispatch_pane_scroll_offset(pane_id, offset_from_bottom, outcome);
    }

    fn dispatch_pane_scroll_offset(
        &mut self,
        pane_id: String,
        offset_from_bottom: usize,
        outcome: &mut ClientShellInput,
    ) {
        if self.snapshot.is_none() {
            return;
        }
        self.next_scroll_serial = self.next_scroll_serial.saturating_add(1);
        let serial = self.next_scroll_serial;
        self.pane_scroll_targets
            .insert(pane_id.clone(), offset_from_bottom);
        self.pane_scroll_in_flight.insert(pane_id.clone(), serial);
        if !self.push_endpoint_method_with_kind(
            crate::api::schema::Method::PaneScroll(crate::api::schema::PaneScrollParams {
                pane_id: pane_id.clone(),
                offset_from_bottom: offset_from_bottom as u64,
            }),
            PendingEndpointKind::PaneScroll {
                pane_id: pane_id.clone(),
                serial,
            },
            outcome,
        ) {
            self.pane_scroll_targets.remove(&pane_id);
            self.pane_scroll_in_flight.remove(&pane_id);
        }
    }

    pub(in crate::client) fn complete_pane_scroll(
        &mut self,
        pane_id: String,
        serial: u64,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.pane_scroll_in_flight.get(&pane_id).copied() != Some(serial) {
            return false;
        }
        self.pane_scroll_in_flight.remove(&pane_id);
        let repaint = match result {
            Ok(crate::api::schema::ResponseResult::PaneInfo { pane })
                if pane.pane_id == pane_id =>
            {
                if let Some(scroll) = pane.scroll {
                    if self.pane_scroll_targets.contains_key(&pane_id) {
                        self.pane_scroll_targets.insert(
                            pane_id.clone(),
                            usize::try_from(scroll.offset_from_bottom).unwrap_or(usize::MAX),
                        );
                    }
                }
                false
            }
            Ok(_) => {
                self.pane_scroll_queued.remove(&pane_id);
                self.pane_scroll_targets.remove(&pane_id);
                tracing::warn!("endpoint returned an unexpected pane-scroll result");
                true
            }
            Err(_) => {
                self.pane_scroll_queued.remove(&pane_id);
                self.pane_scroll_targets.remove(&pane_id);
                true
            }
        };
        if let Some(offset) = self.pane_scroll_queued.remove(&pane_id) {
            self.dispatch_pane_scroll_offset(pane_id, offset, outcome);
        }
        repaint
    }
}
