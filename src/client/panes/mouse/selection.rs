use super::*;

const SELECTION_AUTOSCROLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(30);

impl ClientShellState {
    pub(in crate::client) fn stop_selection_autoscroll(&mut self) {
        self.selection_autoscroll = None;
        self.selection_autoscroll_deadline = None;
    }

    fn selection_edge_scroll_lines(distance: u16) -> usize {
        usize::from(distance).saturating_mul(3).clamp(3, 15)
    }

    fn selection_scroll_metrics(&self, hit: &PaneHit) -> Option<crate::pane::ScrollMetrics> {
        let metrics = hit.scroll?;
        Some(
            self.selection_autoscroll
                .as_ref()
                .filter(|autoscroll| autoscroll.pane_id == hit.pane_id)
                .map_or(metrics, |autoscroll| crate::pane::ScrollMetrics {
                    offset_from_bottom: autoscroll.offset_from_bottom,
                    max_offset_from_bottom: autoscroll.max_offset_from_bottom,
                    viewport_rows: metrics.viewport_rows,
                }),
        )
    }

    fn update_selection_cursor_with_metrics(
        &mut self,
        hit: &PaneHit,
        column: u16,
        row: u16,
        metrics: Option<crate::pane::ScrollMetrics>,
    ) {
        if let Some(selection) = self.selection.as_mut() {
            selection.drag(column, row, hit.inner_rect, metrics);
        }
    }

    pub(super) fn update_selection_drag(
        &mut self,
        hit: &PaneHit,
        column: u16,
        row: u16,
        outcome: &mut ClientShellInput,
    ) {
        let metrics = self.selection_scroll_metrics(hit);
        let was_dragging = self
            .selection
            .as_ref()
            .is_some_and(crate::selection::Selection::is_dragging);
        let moved_from_anchor = self.selection.as_ref().is_some_and(|selection| {
            let (anchor_row, anchor_col) = selection.anchor_screen_pos(hit.inner_rect, metrics);
            anchor_row != row || anchor_col != column
        });
        let is_dragging = was_dragging || moved_from_anchor;
        self.update_selection_cursor_with_metrics(hit, column, row, metrics);
        if is_dragging {
            if let Some(selection) = self.selection.as_mut() {
                if selection.is_just_click() {
                    selection.force_dragging();
                }
            }
            self.last_pane_click = None;
        }
        if !is_dragging {
            self.stop_selection_autoscroll();
            return;
        }

        let Some(metrics) = metrics else {
            self.stop_selection_autoscroll();
            return;
        };
        let top = hit.inner_rect.y;
        let bottom = hit.inner_rect.y + hit.inner_rect.height.saturating_sub(1);
        let (direction, immediate_lines) = if row < top {
            (
                ClientSelectionAutoscrollDirection::Up,
                Self::selection_edge_scroll_lines(top - row),
            )
        } else if row > bottom {
            (
                ClientSelectionAutoscrollDirection::Down,
                Self::selection_edge_scroll_lines(row - bottom),
            )
        } else if row == top {
            (ClientSelectionAutoscrollDirection::Up, 0)
        } else if row == bottom {
            (ClientSelectionAutoscrollDirection::Down, 0)
        } else {
            self.stop_selection_autoscroll();
            return;
        };

        let offset_from_bottom = match direction {
            ClientSelectionAutoscrollDirection::Up => metrics
                .offset_from_bottom
                .saturating_add(immediate_lines)
                .min(metrics.max_offset_from_bottom),
            ClientSelectionAutoscrollDirection::Down => {
                metrics.offset_from_bottom.saturating_sub(immediate_lines)
            }
        };
        if offset_from_bottom != metrics.offset_from_bottom {
            let projected = crate::pane::ScrollMetrics {
                offset_from_bottom,
                ..metrics
            };
            self.update_selection_cursor_with_metrics(hit, column, row, Some(projected));
            self.push_pane_scroll_offset(hit.pane_id.clone(), offset_from_bottom, outcome);
        }
        self.selection_autoscroll = Some(ClientSelectionAutoscroll {
            pane_id: hit.pane_id.clone(),
            direction,
            last_mouse_column: column,
            last_mouse_row: row,
            inner_rect: hit.inner_rect,
            offset_from_bottom,
            max_offset_from_bottom: metrics.max_offset_from_bottom,
        });
        self.selection_autoscroll_deadline =
            Some(std::time::Instant::now() + SELECTION_AUTOSCROLL_INTERVAL);
    }

    pub(super) fn scroll_in_progress_selection(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) || !self
            .selection
            .as_ref()
            .is_some_and(crate::selection::Selection::is_in_progress)
        {
            return false;
        }
        let Some(hit) = self.selection.as_ref().and_then(|selection| {
            self.hits
                .panes
                .iter()
                .find(|hit| hit.pane_id == selection.pane_id)
                .cloned()
        }) else {
            return false;
        };
        let Some(metrics) = self.selection_scroll_metrics(&hit) else {
            return false;
        };
        let offset_from_bottom = match mouse.kind {
            MouseEventKind::ScrollUp => metrics
                .offset_from_bottom
                .saturating_add(self.config.mouse_scroll_lines)
                .min(metrics.max_offset_from_bottom),
            MouseEventKind::ScrollDown => metrics
                .offset_from_bottom
                .saturating_sub(self.config.mouse_scroll_lines),
            _ => unreachable!(),
        };
        if offset_from_bottom != metrics.offset_from_bottom {
            let projected = crate::pane::ScrollMetrics {
                offset_from_bottom,
                ..metrics
            };
            self.update_selection_cursor_with_metrics(
                &hit,
                mouse.column,
                mouse.row,
                Some(projected),
            );
            self.push_pane_scroll_offset(hit.pane_id, offset_from_bottom, outcome);
            outcome.repaint = true;
        }
        true
    }

    pub(crate) fn tick_selection_autoscroll(
        &mut self,
        now: std::time::Instant,
    ) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        if self
            .selection_autoscroll_deadline
            .is_none_or(|deadline| now < deadline)
        {
            return outcome;
        }
        let Some(mut autoscroll) = self.selection_autoscroll.clone() else {
            self.selection_autoscroll_deadline = None;
            return outcome;
        };
        if !self.selection.as_ref().is_some_and(|selection| {
            selection.pane_id == autoscroll.pane_id && selection.is_dragging()
        }) {
            self.stop_selection_autoscroll();
            return outcome;
        }
        let Some(hit) = self
            .hits
            .panes
            .iter()
            .find(|hit| hit.pane_id == autoscroll.pane_id)
            .cloned()
        else {
            self.stop_selection_autoscroll();
            return outcome;
        };
        if hit.inner_rect != autoscroll.inner_rect {
            self.stop_selection_autoscroll();
            return outcome;
        }
        let next_offset = match autoscroll.direction {
            ClientSelectionAutoscrollDirection::Up => autoscroll
                .offset_from_bottom
                .saturating_add(1)
                .min(autoscroll.max_offset_from_bottom),
            ClientSelectionAutoscrollDirection::Down => {
                autoscroll.offset_from_bottom.saturating_sub(1)
            }
        };
        if next_offset == autoscroll.offset_from_bottom {
            self.stop_selection_autoscroll();
            return outcome;
        }
        autoscroll.offset_from_bottom = next_offset;
        let metrics = crate::pane::ScrollMetrics {
            offset_from_bottom: next_offset,
            max_offset_from_bottom: autoscroll.max_offset_from_bottom,
            viewport_rows: hit.scroll.map_or(0, |metrics| metrics.viewport_rows),
        };
        self.update_selection_cursor_with_metrics(
            &hit,
            autoscroll.last_mouse_column,
            autoscroll.last_mouse_row,
            Some(metrics),
        );
        self.push_pane_scroll_offset(autoscroll.pane_id.clone(), next_offset, &mut outcome);
        self.selection_autoscroll = Some(autoscroll);
        self.selection_autoscroll_deadline = Some(now + SELECTION_AUTOSCROLL_INTERVAL);
        outcome.repaint = true;
        outcome
    }
}
