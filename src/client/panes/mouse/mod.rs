//! Ordered mouse dispatch; helpers own geometry, selection, scrolling, and forwarding.
mod forward;
mod hit;
mod scroll;
mod selection;
mod splits;

use crate::client::compositor::{
    contains, pane_surface_topology_signature, push_target_event, ClientChromeDrag,
    ClientInputTarget, ClientPaneClick, ClientPaneMouseGesture, ClientSelectionAutoscroll,
    ClientSelectionAutoscrollDirection, ClientShellEndpointError, ClientShellInput,
    ClientShellState, PaneHit, PaneSplitHit, PendingEndpointKind,
};
use crate::protocol::{ClientMousePosition, ClientPaneInputEvent};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

impl ClientShellState {
    pub(in crate::client) fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) {
        if self.consume_url_mouse(mouse)
            || self.continue_pane_mouse_gesture(mouse, outcome)
            || self.activate_pane_link(mouse, outcome)
            || self.continue_chrome_drag(mouse, outcome)
            || self.finish_chrome_drag(mouse, outcome)
            || self.handle_selection_mouse(mouse, outcome)
        {
            return;
        }
        self.dispatch_pane_mouse(mouse, outcome);
    }

    fn consume_url_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self.url_click_consumes_until_up {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => return true,
                MouseEventKind::Up(MouseButton::Left) => {
                    self.url_click_consumes_until_up = false;
                    return true;
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
                return true;
            }
        }
        false
    }

    fn continue_pane_mouse_gesture(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if let Some(gesture) = self.pane_mouse_gesture.as_ref() {
            let gesture_event = matches!(
                mouse.kind,
                MouseEventKind::Drag(button) | MouseEventKind::Up(button)
                    if button == gesture.button
            );
            if gesture_event {
                let button = gesture.button;
                let modifiers = mouse.modifiers.difference(gesture.stripped_modifiers);
                let hit = self
                    .hits
                    .panes
                    .iter()
                    .find(|hit| hit.pane_id == gesture.hit.pane_id)
                    .cloned()
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
                return true;
            }
            if matches!(
                mouse.kind,
                MouseEventKind::Down(_) | MouseEventKind::Drag(_) | MouseEventKind::Up(_)
            ) {
                return true;
            }
        }
        false
    }

    fn activate_pane_link(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) -> bool {
        let point = (mouse.column, mouse.row);
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
                .find(|hit| contains(hit.inner_rect, point))
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
                return true;
            }
        }
        false
    }

    fn continue_chrome_drag(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) -> bool {
        if mouse.kind != MouseEventKind::Drag(MouseButton::Left) {
            return false;
        }
        match self.chrome_drag.as_ref() {
            Some(ClientChromeDrag::PaneScrollbar { .. }) => {
                self.drag_pane_scrollbar(mouse, outcome)
            }
            Some(ClientChromeDrag::PaneSplit { .. }) => self.drag_pane_split(mouse, outcome),
            None => return false,
        }
        true
    }

    fn drag_pane_scrollbar(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        let Some(ClientChromeDrag::PaneScrollbar {
            hit,
            grab_row_offset,
            last_sent_offset,
            last_sent_at,
        }) = self.chrome_drag.as_ref()
        else {
            return;
        };
        let current_hit = self
            .hits
            .panes
            .iter()
            .find(|current| current.pane_id == hit.pane_id)
            .cloned()
            .unwrap_or_else(|| hit.clone());
        let Some(offset) =
            Self::pane_scrollbar_offset(&current_hit, mouse.row, Some(*grab_row_offset))
        else {
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
    }

    fn drag_pane_split(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        let point = (mouse.column, mouse.row);
        let Some(ClientChromeDrag::PaneSplit {
            hit,
            tab_id,
            grab_offset,
            last_sent_at,
            ..
        }) = self.chrome_drag.as_ref()
        else {
            return;
        };
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
        let should_send = last_sent_at
            .is_none_or(|last| now.duration_since(last) >= std::time::Duration::from_millis(33));
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
    }

    fn finish_chrome_drag(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) -> bool {
        let point = (mouse.column, mouse.row);
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
                return true;
            }
        }
        false
    }

    fn handle_selection_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
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
                return true;
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
            return true;
        }
        if self.scroll_in_progress_selection(mouse, outcome) {
            return true;
        }
        false
    }

    fn dispatch_pane_mouse(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        let point = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => {
                self.handle_pane_right_click(mouse, outcome)
            }
            MouseEventKind::Down(MouseButton::Left) => self.handle_pane_left_click(mouse, outcome),
            MouseEventKind::Down(MouseButton::Middle) => {
                if let Some(hit) = self
                    .hits
                    .panes
                    .iter()
                    .find(|hit| contains(hit.inner_rect, point) && hit.mouse_reporting)
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
                    .find(|hit| contains(hit.inner_rect, point) && hit.mouse_reporting)
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
                    .find(|hit| contains(hit.inner_rect, point))
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

    fn handle_pane_right_click(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        let point = (mouse.column, mouse.row);
        let pane_hit = self
            .hits
            .panes
            .iter()
            .find(|hit| contains(hit.inner_rect, point))
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
            if hit.mouse_reporting && (pane_owns_right_click || configured_modifiers.is_some()) {
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

    fn handle_pane_left_click(&mut self, mouse: MouseEvent, outcome: &mut ClientShellInput) {
        let point = (mouse.column, mouse.row);
        if self.selection.take().is_some() {
            outcome.repaint = true;
        }
        self.stop_selection_autoscroll();
        self.selection_highlight_clear_deadline = None;
        self.pending_word_selection = None;
        let previous_pane_click = self.last_pane_click.take();
        self.chrome_drag = None;
        if self.start_pane_scrollbar_drag(mouse, outcome) || self.start_pane_split_drag(mouse) {
            return;
        }
        let pane_hit = self
            .hits
            .panes
            .iter()
            .find(|hit| contains(hit.rect, point))
            .cloned();
        if let Some(hit) = pane_hit {
            if hit.mouse_reporting && contains(hit.inner_rect, point) {
                self.push_pane_mouse_event(&hit, mouse, mouse.modifiers, outcome);
                self.pane_mouse_gesture = Some(ClientPaneMouseGesture {
                    last_position: self.pane_mouse_position(&hit, mouse),
                    hit: hit.clone(),
                    button: MouseButton::Left,
                    stripped_modifiers: crossterm::event::KeyModifiers::empty(),
                    last_event: mouse,
                });
            } else if contains(hit.inner_rect, point) {
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
                    self.request_word_selection(&hit, click.viewport_row, click.col, outcome);
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

    fn start_pane_scrollbar_drag(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let point = (mouse.column, mouse.row);
        let scrollbar_hit = self
            .hits
            .panes
            .iter()
            .find(|hit| {
                hit.scrollbar_rect.is_some_and(|rect| contains(rect, point))
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
                return true;
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
            } else if let Some(offset) = Self::pane_scrollbar_offset(&hit, mouse.row, None) {
                self.push_pane_scroll_offset(hit.pane_id, offset, outcome);
            }
            return true;
        }
        false
    }

    fn start_pane_split_drag(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row);
        let split_hit = self
            .hits
            .pane_splits
            .iter()
            .find(|hit| contains(hit.hit_rect, point))
            .cloned();
        if let Some(hit) = split_hit {
            let Some(tab_id) = self
                .snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.focused_tab_id.clone())
            else {
                return true;
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
            return true;
        }
        false
    }
}
