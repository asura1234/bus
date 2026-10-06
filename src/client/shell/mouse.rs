use super::*;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

const SELECTION_AUTOSCROLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(30);

impl ClientShellState {
    fn pane_scrollbar_offset(
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

    pub(super) fn push_pane_scroll_offset(
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

    pub(super) fn complete_pane_scroll(
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
                self.endpoint_error =
                    Some("endpoint returned an unexpected pane-scroll result".to_owned());
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

    pub(super) fn stop_selection_autoscroll(&mut self) {
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

    fn update_selection_drag(
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

    fn scroll_in_progress_selection(
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

    fn pane_split_target_is_current(&self, hit: &PaneSplitHit, tab_id: &str) -> Option<bool> {
        let snapshot = self.snapshot.as_deref()?;
        let surface = self.pane_surface.as_ref()?;
        if snapshot.revision != surface.projection_revision {
            return None;
        }
        Some(
            snapshot.focused_tab_id.as_deref() == Some(tab_id)
                && pane_surface_topology_signature(surface) == hit.topology_signature,
        )
    }

    fn pane_split_ratio(hit: &PaneSplitHit, grab_offset: i32, point: (u16, u16)) -> f32 {
        let (pointer, origin, length) = match hit.direction {
            crate::protocol::PaneSurfaceSplitDirection::Horizontal => {
                (i32::from(point.0), i32::from(hit.area.x), hit.area.width)
            }
            crate::protocol::PaneSurfaceSplitDirection::Vertical => {
                (i32::from(point.1), i32::from(hit.area.y), hit.area.height)
            }
        };
        ((pointer + grab_offset - origin) as f32 / f32::from(length.max(1))).clamp(0.1, 0.9)
    }

    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        let point = (mouse.column, mouse.row);
        if self.url_click_consumes_until_up {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => return,
                MouseEventKind::Up(MouseButton::Left) => {
                    self.url_click_consumes_until_up = false;
                    return;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    self.url_click_consumes_until_up = false;
                }
                _ => {}
            }
        }
        if !self.replaying_url_click
            && matches!(
                mouse.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            )
        {
            if let Some(fallback_events) =
                self.pending_requests
                    .values_mut()
                    .find_map(|pending| match &mut pending.kind {
                        PendingEndpointKind::PaneLinkActivate {
                            fallback_events, ..
                        } if !fallback_events
                            .iter()
                            .any(|event| event.kind == MouseEventKind::Up(MouseButton::Left)) =>
                        {
                            Some(fallback_events)
                        }
                        _ => None,
                    })
            {
                fallback_events.push(mouse);
                return;
            }
        }
        if let Some(gesture) = self.pane_mouse_gesture.as_ref() {
            let gesture_event = matches!(
                mouse.kind,
                MouseEventKind::Drag(button) | MouseEventKind::Up(button)
                    if button == gesture.button
            );
            if gesture_event {
                let button = gesture.button;
                let modifiers = mouse.modifiers.difference(gesture.stripped_modifiers);
                let hit = if gesture.hit.popup {
                    self.hits
                        .popup
                        .as_ref()
                        .filter(|hit| hit.pane_id == gesture.hit.pane_id)
                        .cloned()
                } else {
                    self.hits
                        .panes
                        .iter()
                        .find(|hit| hit.pane_id == gesture.hit.pane_id)
                        .cloned()
                }
                .unwrap_or_else(|| gesture.hit.clone());
                let position = self.pane_mouse_position(&hit, mouse);
                if let Some(gesture) = self.pane_mouse_gesture.as_mut() {
                    gesture.last_event = mouse;
                    gesture.last_position = position;
                }
                self.push_pane_mouse_event(&hit, mouse, modifiers, outcome);
                if mouse.kind == MouseEventKind::Up(button) {
                    self.pane_mouse_gesture = None;
                }
                return;
            }
            if matches!(
                mouse.kind,
                MouseEventKind::Down(_) | MouseEventKind::Drag(_) | MouseEventKind::Up(_)
            ) {
                return;
            }
        }
        if let Some(hit) = self.hits.popup.clone() {
            if super::contains(hit.inner_rect, point) {
                match mouse.kind {
                    MouseEventKind::Down(button) => {
                        self.push_pane_mouse_event(&hit, mouse, mouse.modifiers, outcome);
                        if hit.mouse_reporting {
                            self.pane_mouse_gesture = Some(ClientPaneMouseGesture {
                                last_position: self.pane_mouse_position(&hit, mouse),
                                hit,
                                button,
                                stripped_modifiers: crossterm::event::KeyModifiers::empty(),
                                last_event: mouse,
                            });
                        }
                    }
                    MouseEventKind::Moved if hit.mouse_reporting => {
                        self.push_pane_mouse_event(&hit, mouse, mouse.modifiers, outcome);
                    }
                    MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
                    | MouseEventKind::ScrollLeft
                    | MouseEventKind::ScrollRight => {
                        self.push_pane_mouse_event(&hit, mouse, mouse.modifiers, outcome);
                    }
                    MouseEventKind::Up(_) | MouseEventKind::Drag(_) | MouseEventKind::Moved => {}
                }
            }
            return;
        }
        if self.popup_terminal_id.is_some() {
            return;
        }
        if !self.replaying_url_click
            && mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && mouse
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            if let Some(hit) = self
                .hits
                .panes
                .iter()
                .find(|hit| super::contains(hit.inner_rect, point))
                .cloned()
            {
                let viewport_row = mouse.row.saturating_sub(hit.inner_rect.y);
                let col = mouse.column.saturating_sub(hit.inner_rect.x);
                let content_revision = self
                    .pane_surface
                    .as_ref()
                    .and_then(|surface| {
                        surface
                            .panes
                            .iter()
                            .find(|pane| pane.pane_id == hit.pane_id)
                    })
                    .map(|pane| pane.content_revision);
                self.last_pane_click = None;
                let pane_id = hit.pane_id.clone();
                self.push_endpoint_method_with_kind(
                    crate::api::schema::Method::PaneLinkActivate(
                        crate::api::schema::PaneLinkActivateParams {
                            pane_id: pane_id.clone(),
                            viewport_row,
                            col,
                            content_revision,
                            offset_from_bottom: hit
                                .scroll
                                .map(|metrics| metrics.offset_from_bottom as u64),
                        },
                    ),
                    PendingEndpointKind::PaneLinkActivate {
                        pane_id,
                        inner_rect: hit.inner_rect,
                        fallback_events: vec![mouse],
                    },
                    outcome,
                );
                return;
            }
        }
        if self.visible_endpoint_notice.is_some()
            && mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && super::contains(self.hits.notification_toast, point)
        {
            self.visible_endpoint_notice = None;
            outcome.repaint = true;
            return;
        }
        if mouse.kind == MouseEventKind::Drag(MouseButton::Left) {
            match self.chrome_drag.as_ref() {
                Some(ClientChromeDrag::PaneScrollbar {
                    hit,
                    grab_row_offset,
                    last_sent_offset,
                    last_sent_at,
                }) => {
                    let current_hit = self
                        .hits
                        .panes
                        .iter()
                        .find(|current| current.pane_id == hit.pane_id)
                        .cloned()
                        .unwrap_or_else(|| hit.clone());
                    let Some(offset) = Self::pane_scrollbar_offset(
                        &current_hit,
                        mouse.row,
                        Some(*grab_row_offset),
                    ) else {
                        self.chrome_drag = None;
                        return;
                    };
                    let now = std::time::Instant::now();
                    let should_send = *last_sent_offset != Some(offset)
                        && last_sent_at.is_none_or(|last| {
                            now.duration_since(last) >= std::time::Duration::from_millis(33)
                        });
                    if should_send {
                        if let Some(ClientChromeDrag::PaneScrollbar {
                            last_sent_offset,
                            last_sent_at,
                            ..
                        }) = self.chrome_drag.as_mut()
                        {
                            *last_sent_offset = Some(offset);
                            *last_sent_at = Some(now);
                        }
                        self.push_pane_scroll_offset(current_hit.pane_id, offset, outcome);
                    }
                    return;
                }
                Some(ClientChromeDrag::PaneSplit {
                    hit,
                    tab_id,
                    grab_offset,
                    last_sent_at,
                    ..
                }) => {
                    let hit = hit.clone();
                    let tab_id = tab_id.clone();
                    let grab_offset = *grab_offset;
                    match self.pane_split_target_is_current(&hit, &tab_id) {
                        Some(true) => {}
                        Some(false) => {
                            self.chrome_drag = None;
                            return;
                        }
                        None => return,
                    }
                    let ratio = Self::pane_split_ratio(&hit, grab_offset, point);
                    let now = std::time::Instant::now();
                    let should_send = last_sent_at.is_none_or(|last| {
                        now.duration_since(last) >= std::time::Duration::from_millis(33)
                    });
                    if let Some(ClientChromeDrag::PaneSplit {
                        last_sent_ratio,
                        last_sent_at,
                        ..
                    }) = self.chrome_drag.as_mut()
                    {
                        if should_send {
                            *last_sent_ratio = Some(ratio);
                            *last_sent_at = Some(now);
                        }
                    }
                    if should_send {
                        self.push_endpoint_method(
                            crate::api::schema::Method::LayoutSetSplitRatio(
                                crate::api::schema::LayoutSetSplitRatioParams {
                                    tab_id: Some(tab_id),
                                    pane_id: None,
                                    path: hit.path,
                                    ratio,
                                },
                            ),
                            outcome,
                        );
                    }
                    return;
                }
                None => {}
            }
        }
        if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
            if let Some(drag) = self.chrome_drag.take() {
                match drag {
                    ClientChromeDrag::PaneScrollbar {
                        hit,
                        grab_row_offset,
                        last_sent_offset,
                        ..
                    } => {
                        let current_hit = self
                            .hits
                            .panes
                            .iter()
                            .find(|current| current.pane_id == hit.pane_id)
                            .cloned()
                            .unwrap_or(hit);
                        if let Some(offset) = Self::pane_scrollbar_offset(
                            &current_hit,
                            mouse.row,
                            Some(grab_row_offset),
                        ) {
                            if last_sent_offset != Some(offset) {
                                self.push_pane_scroll_offset(current_hit.pane_id, offset, outcome);
                            }
                        }
                    }
                    ClientChromeDrag::PaneSplit {
                        hit,
                        tab_id,
                        grab_offset,
                        last_sent_ratio,
                        ..
                    } => {
                        let target_is_current =
                            self.pane_split_target_is_current(&hit, &tab_id) == Some(true);
                        let ratio = Self::pane_split_ratio(&hit, grab_offset, point);
                        if target_is_current
                            && last_sent_ratio
                                .is_none_or(|sent| (sent - ratio).abs() > f32::EPSILON)
                        {
                            self.push_endpoint_method(
                                crate::api::schema::Method::LayoutSetSplitRatio(
                                    crate::api::schema::LayoutSetSplitRatioParams {
                                        tab_id: Some(tab_id),
                                        pane_id: None,
                                        path: hit.path,
                                        ratio,
                                    },
                                ),
                                outcome,
                            );
                        }
                    }
                }
                return;
            }
        }
        if mouse.kind == MouseEventKind::Drag(MouseButton::Left) {
            let selection_hit = self.selection.as_ref().and_then(|selection| {
                self.hits
                    .panes
                    .iter()
                    .find(|hit| hit.pane_id == selection.pane_id)
                    .cloned()
            });
            if let Some(hit) = selection_hit {
                self.update_selection_drag(&hit, mouse.column, mouse.row, outcome);
                outcome.repaint = true;
                return;
            }
        }
        if mouse.kind == MouseEventKind::Up(MouseButton::Left) && self.selection.is_some() {
            self.stop_selection_autoscroll();
            let copied = self
                .selection
                .as_mut()
                .is_some_and(crate::selection::Selection::finish);
            if copied && self.config.copy_on_select {
                self.request_selection_copy(outcome, false);
                self.selection = None;
            } else if !copied {
                self.selection = None;
            }
            if copied {
                self.last_pane_click = None;
            }
            outcome.repaint = true;
            return;
        }
        if self.scroll_in_progress_selection(mouse, outcome) {
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                let pane_hit = self
                    .hits
                    .panes
                    .iter()
                    .find(|hit| super::contains(hit.inner_rect, point))
                    .cloned();
                if let Some(hit) = pane_hit {
                    let pane_owns_right_click = self
                        .snapshot
                        .as_deref()
                        .and_then(|snapshot| {
                            snapshot
                                .panes
                                .iter()
                                .find(|pane| pane.pane_id == hit.pane_id)
                        })
                        .is_some_and(|pane| pane.right_click_passthrough)
                        && mouse.modifiers.is_empty();
                    let configured_modifiers = self
                        .config
                        .right_click_passthrough_modifiers
                        .filter(|modifiers| *modifiers == mouse.modifiers);
                    if hit.mouse_reporting
                        && (pane_owns_right_click || configured_modifiers.is_some())
                    {
                        let stripped_modifiers =
                            configured_modifiers.unwrap_or(crossterm::event::KeyModifiers::empty());
                        self.push_pane_mouse_event(
                            &hit,
                            mouse,
                            mouse.modifiers.difference(stripped_modifiers),
                            outcome,
                        );
                        self.push_endpoint_method(
                            crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget {
                                pane_id: hit.pane_id.clone(),
                            }),
                            outcome,
                        );
                        self.pane_mouse_gesture = Some(ClientPaneMouseGesture {
                            last_position: self.pane_mouse_position(&hit, mouse),
                            hit,
                            button: MouseButton::Right,
                            stripped_modifiers,
                            last_event: mouse,
                        });
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.selection.take().is_some() {
                    outcome.repaint = true;
                }
                self.stop_selection_autoscroll();
                self.selection_highlight_clear_deadline = None;
                self.pending_word_selection = None;
                let previous_pane_click = self.last_pane_click.take();
                self.chrome_drag = None;
                let scrollbar_hit = self
                    .hits
                    .panes
                    .iter()
                    .find(|hit| {
                        hit.scrollbar_rect
                            .is_some_and(|rect| super::contains(rect, point))
                            && hit
                                .scroll
                                .is_some_and(|metrics| metrics.max_offset_from_bottom > 0)
                    })
                    .cloned();
                if let Some(hit) = scrollbar_hit {
                    self.push_endpoint_method(
                        crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget {
                            pane_id: hit.pane_id.clone(),
                        }),
                        outcome,
                    );
                    let (Some(track), Some(metrics)) = (hit.scrollbar_rect, hit.scroll) else {
                        return;
                    };
                    if let Some(grab_row_offset) =
                        crate::ui::scrollbar_thumb_grab_offset(metrics, track, mouse.row)
                    {
                        self.chrome_drag = Some(ClientChromeDrag::PaneScrollbar {
                            hit,
                            grab_row_offset,
                            last_sent_offset: None,
                            last_sent_at: None,
                        });
                    } else if let Some(offset) = Self::pane_scrollbar_offset(&hit, mouse.row, None)
                    {
                        self.push_pane_scroll_offset(hit.pane_id, offset, outcome);
                    }
                    return;
                }
                let split_hit = self
                    .hits
                    .pane_splits
                    .iter()
                    .find(|hit| super::contains(hit.hit_rect, point))
                    .cloned();
                if let Some(hit) = split_hit {
                    let Some(tab_id) = self
                        .snapshot
                        .as_deref()
                        .and_then(|snapshot| snapshot.focused_tab_id.clone())
                    else {
                        return;
                    };
                    let pointer = match hit.direction {
                        crate::protocol::PaneSurfaceSplitDirection::Horizontal => mouse.column,
                        crate::protocol::PaneSurfaceSplitDirection::Vertical => mouse.row,
                    };
                    self.chrome_drag = Some(ClientChromeDrag::PaneSplit {
                        grab_offset: i32::from(hit.pos) - i32::from(pointer),
                        last_sent_ratio: None,
                        last_sent_at: None,
                        hit,
                        tab_id,
                    });
                    return;
                }
                let pane_hit = self
                    .hits
                    .panes
                    .iter()
                    .find(|hit| super::contains(hit.rect, point))
                    .cloned();
                if let Some(hit) = pane_hit {
                    if hit.mouse_reporting && super::contains(hit.inner_rect, point) {
                        self.push_pane_mouse_event(&hit, mouse, mouse.modifiers, outcome);
                        self.pane_mouse_gesture = Some(ClientPaneMouseGesture {
                            last_position: self.pane_mouse_position(&hit, mouse),
                            hit: hit.clone(),
                            button: MouseButton::Left,
                            stripped_modifiers: crossterm::event::KeyModifiers::empty(),
                            last_event: mouse,
                        });
                    } else if super::contains(hit.inner_rect, point) {
                        let click = ClientPaneClick {
                            pane_id: hit.pane_id.clone(),
                            viewport_row: mouse.row.saturating_sub(hit.inner_rect.y),
                            col: mouse.column.saturating_sub(hit.inner_rect.x),
                            at: std::time::Instant::now(),
                        };
                        if mouse.modifiers.is_empty()
                            && previous_pane_click
                                .as_ref()
                                .is_some_and(|previous| previous.is_double_click_for(&click))
                        {
                            self.request_word_selection(
                                &hit,
                                click.viewport_row,
                                click.col,
                                outcome,
                            );
                        } else {
                            if mouse.modifiers.is_empty() {
                                self.last_pane_click = Some(click);
                            }
                            self.selection = Some(crate::selection::Selection::anchor(
                                hit.pane_id.clone(),
                                mouse.row.saturating_sub(hit.inner_rect.y),
                                mouse.column.saturating_sub(hit.inner_rect.x),
                                hit.scroll,
                            ));
                        }
                    }
                    self.push_endpoint_method(
                        crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget {
                            pane_id: hit.pane_id,
                        }),
                        outcome,
                    );
                }
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                if let Some(hit) = self
                    .hits
                    .panes
                    .iter()
                    .find(|hit| super::contains(hit.inner_rect, point) && hit.mouse_reporting)
                    .cloned()
                {
                    self.push_pane_mouse_event(&hit, mouse, mouse.modifiers, outcome);
                    self.pane_mouse_gesture = Some(ClientPaneMouseGesture {
                        last_position: self.pane_mouse_position(&hit, mouse),
                        hit,
                        button: MouseButton::Middle,
                        stripped_modifiers: crossterm::event::KeyModifiers::empty(),
                        last_event: mouse,
                    });
                }
            }
            MouseEventKind::Up(MouseButton::Left | MouseButton::Middle)
            | MouseEventKind::Drag(MouseButton::Left | MouseButton::Middle) => {}
            MouseEventKind::Moved => {
                if let Some(hit) = self
                    .hits
                    .panes
                    .iter()
                    .find(|hit| super::contains(hit.inner_rect, point) && hit.mouse_reporting)
                    .cloned()
                {
                    self.push_pane_mouse_event(&hit, mouse, mouse.modifiers, outcome);
                }
            }
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {
                if let Some(hit) = self
                    .hits
                    .panes
                    .iter()
                    .find(|hit| super::contains(hit.inner_rect, point))
                    .cloned()
                {
                    if self.focused_pane_id().as_deref() != Some(hit.pane_id.as_str()) {
                        self.push_endpoint_method(
                            crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget {
                                pane_id: hit.pane_id.clone(),
                            }),
                            outcome,
                        );
                    }
                    self.push_pane_mouse_event(&hit, mouse, mouse.modifiers, outcome);
                }
            }
            _ => {}
        }
    }

    fn pane_mouse_position(&self, hit: &PaneHit, mouse: MouseEvent) -> ClientMousePosition {
        let cell = ClientMousePosition::Cell {
            column: mouse.column.saturating_sub(hit.inner_rect.x),
            row: mouse.row.saturating_sub(hit.inner_rect.y),
        };
        if hit.sgr_pixel_mouse && hit.pixel_width > 0 && hit.pixel_height > 0 {
            self.host_mouse_pixels
                .and_then(|pixels| {
                    pixels
                        .pane_position(hit.inner_rect, hit.pixel_width, hit.pixel_height)
                        .and_then(|position| match position {
                            crate::input::mouse::Position::Pixels { x, y } => {
                                Some(ClientMousePosition::Pixels {
                                    x,
                                    y,
                                    column: mouse.column.saturating_sub(hit.inner_rect.x),
                                    row: mouse.row.saturating_sub(hit.inner_rect.y),
                                })
                            }
                            crate::input::mouse::Position::Cell { .. } => None,
                        })
                })
                .unwrap_or(cell)
        } else {
            cell
        }
    }

    pub(super) fn push_pane_mouse_event(
        &self,
        hit: &PaneHit,
        mouse: MouseEvent,
        modifiers: crossterm::event::KeyModifiers,
        outcome: &mut ClientShellInput,
    ) {
        let Some(kind) = crate::protocol::ClientMouseKind::from_crossterm(mouse.kind) else {
            return;
        };
        let position = self.pane_mouse_position(hit, mouse);
        let geometry = matches!(position, ClientMousePosition::Pixels { .. }).then_some(
            crate::protocol::ClientMouseGeometry {
                cols: hit.inner_rect.width,
                rows: hit.inner_rect.height,
                width_px: hit.pixel_width,
                height_px: hit.pixel_height,
            },
        );
        let target = if hit.popup {
            ClientInputTarget::Popup(hit.pane_id.clone())
        } else {
            ClientInputTarget::Pane(hit.pane_id.clone())
        };
        push_target_event(
            target,
            ClientPaneInputEvent::Mouse {
                kind,
                position,
                geometry,
                modifiers: modifiers.bits(),
                lines: self.config.mouse_scroll_lines.min(u16::MAX as usize) as u16,
            },
            outcome,
        );
    }
}
