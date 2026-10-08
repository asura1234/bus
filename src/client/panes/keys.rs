//! Physical-key leases, releases, and repeat routing.
use crate::client::compositor::{
    push_target_event, ClientInputContext, ClientInputTarget, ClientShellInput, ClientShellState,
};
use crate::protocol::ClientPaneInputEvent;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

const LOCAL_INPUT_SOURCE: u8 = 0;

fn is_retained_selection_copy_key(key: &crate::input::TerminalKey) -> bool {
    matches!(key.code, KeyCode::Char('c' | 'C'))
        && matches!(key.modifiers, KeyModifiers::CONTROL | KeyModifiers::SUPER)
}

impl ClientShellState {
    pub(in crate::client) fn handle_key(
        &mut self,
        key: crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        let lease_key = crate::input::InputLeaseKey::new(LOCAL_INPUT_SOURCE, &key);
        let key = self.input_leases.normalize_press(&lease_key, key);
        match key.kind {
            KeyEventKind::Press => {
                let initial_context = self.input_context();
                let target = self.route_key_press(&key, outcome);
                if let Some(target) = target.as_ref() {
                    self.push_pane_key(target.clone(), key.clone(), outcome);
                }
                let resulting_context = self.input_context();
                let plan = self.input_leases.complete_press(
                    lease_key,
                    &key,
                    Some(&initial_context),
                    Some(&resulting_context),
                    target,
                );
                self.execute_repeat_plan(lease_key, key, plan, outcome);
            }
            KeyEventKind::Repeat => {
                let context = self.input_context();
                let plan = self
                    .input_leases
                    .plan_repeat(lease_key, &key, Some(&context));
                self.execute_repeat_plan(lease_key, key, plan, outcome);
            }
            KeyEventKind::Release => {
                if let Some(lease) = self.input_leases.remove_forwarded(&lease_key) {
                    let release = lease
                        .key
                        .with_modifiers(key.modifiers)
                        .with_kind(KeyEventKind::Release);
                    self.push_pane_key(lease.target, release, outcome);
                } else {
                    let _ = self.input_leases.remove(&lease_key);
                }
            }
        }
    }

    pub(super) fn release_input_leases(&mut self, outcome: &mut ClientShellInput) {
        for lease in self.input_leases.remove_source(LOCAL_INPUT_SOURCE) {
            self.push_pane_key(
                lease.target,
                lease.key.with_kind(KeyEventKind::Release),
                outcome,
            );
        }
        if let Some(gesture) = self.pane_mouse_gesture.take() {
            let modifiers = gesture
                .last_event
                .modifiers
                .difference(gesture.stripped_modifiers);
            let geometry = matches!(
                gesture.last_position,
                crate::protocol::ClientMousePosition::Pixels { .. }
            )
            .then_some(crate::protocol::ClientMouseGeometry {
                cols: gesture.hit.inner_rect.width,
                rows: gesture.hit.inner_rect.height,
                width_px: gesture.hit.pixel_width,
                height_px: gesture.hit.pixel_height,
            });
            let target = ClientInputTarget::Pane(gesture.hit.pane_id);
            push_target_event(
                target,
                ClientPaneInputEvent::Mouse {
                    kind: crate::protocol::ClientMouseKind::Up(
                        crate::protocol::ClientMouseButton::from_crossterm(gesture.button),
                    ),
                    position: gesture.last_position,
                    geometry,
                    modifiers: modifiers.bits(),
                    lines: self.config.mouse_scroll_lines.min(u16::MAX as usize) as u16,
                },
                outcome,
            );
        }
    }

    fn execute_repeat_plan(
        &mut self,
        lease_key: crate::input::InputLeaseKey<u8>,
        key: crate::input::TerminalKey,
        plan: crate::input::RepeatPlan<ClientInputContext, ClientInputTarget>,
        outcome: &mut ClientShellInput,
    ) {
        match plan {
            crate::input::RepeatPlan::Forwarded(target) => {
                self.push_pane_key(target, key, outcome);
            }
            crate::input::RepeatPlan::Reprocess {
                context,
                repetitions,
                tracked,
            } => {
                for _ in 0..repetitions {
                    let current = self.input_context();
                    if !self.input_leases.reprocess_allowed(
                        lease_key,
                        &context,
                        Some(&current),
                        tracked,
                    ) {
                        break;
                    }
                    let repeated = key
                        .clone()
                        .with_repeat_count(1)
                        .with_kind(KeyEventKind::Repeat);
                    if let Some(target) = self.route_key_press(&repeated, outcome) {
                        self.push_pane_key(target, repeated, outcome);
                    }
                }
            }
            crate::input::RepeatPlan::Ignore => {}
        }
    }

    fn route_key_press(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> Option<ClientInputTarget> {
        if matches!(key.code, KeyCode::Modifier(_)) {
            return None;
        }
        self.pending_word_selection = None;
        if !self.config.copy_on_select
            && is_retained_selection_copy_key(key)
            && self
                .selection
                .as_ref()
                .is_some_and(crate::selection::Selection::is_visible)
        {
            self.request_selection_copy(outcome, true);
            self.selection = None;
            self.stop_selection_autoscroll();
            self.selection_highlight_clear_deadline = None;
            outcome.repaint = true;
            return None;
        }
        if self.selection.take().is_some() {
            self.stop_selection_autoscroll();
            self.selection_highlight_clear_deadline = None;
            outcome.repaint = true;
        }
        self.focused_pane_id().map(ClientInputTarget::Pane)
    }

    fn input_context(&self) -> ClientInputContext {
        ClientInputContext {
            retained_selection: self
                .selection
                .as_ref()
                .is_some_and(crate::selection::Selection::is_visible),
        }
    }

    fn push_pane_key(
        &self,
        target: ClientInputTarget,
        key: crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        if let Some(event) = ClientPaneInputEvent::from_terminal_key(key) {
            push_target_event(target, event, outcome);
        }
    }
}
