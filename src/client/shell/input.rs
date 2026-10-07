use super::*;
use crate::protocol::ClientPaneInputEvent;
use crate::raw_input::RawInputEvent;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

const LOCAL_INPUT_SOURCE: u8 = 0;

fn is_retained_selection_copy_key(key: &crate::input::TerminalKey) -> bool {
    matches!(key.code, KeyCode::Char('c' | 'C'))
        && matches!(key.modifiers, KeyModifiers::CONTROL | KeyModifiers::SUPER)
}

fn host_theme_update(event: &RawInputEvent) -> Option<crate::protocol::ClientHostThemeUpdate> {
    use crate::protocol::{
        ClientHostAppearance, ClientHostDefaultColorKind, ClientHostThemeUpdate,
    };

    match event {
        RawInputEvent::HostDefaultColor { kind, color } => {
            Some(ClientHostThemeUpdate::DefaultColor {
                kind: match kind {
                    crate::terminal_theme::DefaultColorKind::Foreground => {
                        ClientHostDefaultColorKind::Foreground
                    }
                    crate::terminal_theme::DefaultColorKind::Background => {
                        ClientHostDefaultColorKind::Background
                    }
                },
                color: (*color).into(),
            })
        }
        RawInputEvent::HostPaletteColors { colors } => Some(ClientHostThemeUpdate::PaletteColors(
            colors
                .iter()
                .map(|(index, color)| (*index, (*color).into()))
                .collect(),
        )),
        RawInputEvent::HostColorSchemeChanged(appearance) => {
            Some(ClientHostThemeUpdate::Appearance(match appearance {
                crate::terminal_theme::HostAppearance::Dark => ClientHostAppearance::Dark,
                crate::terminal_theme::HostAppearance::Light => ClientHostAppearance::Light,
            }))
        }
        _ => None,
    }
}

fn push_host_theme_update(
    requests: &mut Vec<ClientMessage>,
    update: crate::protocol::ClientHostThemeUpdate,
) {
    if let crate::protocol::ClientHostThemeUpdate::PaletteColors(colors) = &update {
        if let Some(ClientMessage::ClientShellHostTheme {
            update: crate::protocol::ClientHostThemeUpdate::PaletteColors(pending),
        }) = requests.last_mut()
        {
            if pending.len() + colors.len() <= 256 {
                pending.extend_from_slice(colors);
                return;
            }
        }
    }
    requests.push(ClientMessage::ClientShellHostTheme { update });
}

impl ClientShellState {
    #[cfg(any(unix, test))]
    pub(crate) fn handle_input_bytes(&mut self, data: &[u8]) -> ClientShellInput {
        self.handle_raw_events(crate::raw_input::parse_raw_input_bytes_sync(data))
    }

    #[cfg(any(unix, test))]
    pub(crate) fn handle_pixel_mouse(
        &mut self,
        data: &[u8],
        geometry: crate::input::mouse::HostGeometry,
    ) -> ClientShellInput {
        let Some((x, y)) = crate::input::mouse::parse_report(data) else {
            return ClientShellInput::default();
        };
        let Some((column, row)) = geometry.cell(x, y) else {
            return ClientShellInput::default();
        };
        let Some(cell_report) = crate::input::mouse::report_at_cell(data, column, row) else {
            return ClientShellInput::default();
        };
        let events = crate::raw_input::parse_raw_input_bytes_sync(&cell_report);
        if events.len() != 1 || !matches!(events[0], RawInputEvent::Mouse(_)) {
            return ClientShellInput::default();
        }
        self.host_mouse_pixels = Some(crate::input::mouse::HostPixels { x, y, geometry });
        let outcome = self.handle_raw_events(events);
        self.host_mouse_pixels = None;
        outcome
    }

    #[cfg(windows)]
    pub(crate) fn handle_client_events(
        &mut self,
        events: &[crate::protocol::ClientInputEvent],
    ) -> ClientShellInput {
        self.handle_raw_events(
            events
                .iter()
                .map(crate::protocol::ClientInputEvent::to_raw_input_event)
                .collect(),
        )
    }

    fn prepare_committed_text(&mut self, outcome: &mut ClientShellInput) {
        self.pending_word_selection = None;
        if self.selection.take().is_some() {
            self.stop_selection_autoscroll();
            self.selection_highlight_clear_deadline = None;
            outcome.repaint = true;
        }
    }

    pub(crate) fn replay_mouse_events(
        &mut self,
        events: Vec<crossterm::event::MouseEvent>,
    ) -> ClientShellInput {
        self.replaying_url_click = true;
        let outcome =
            self.handle_raw_events(events.into_iter().map(RawInputEvent::Mouse).collect());
        self.replaying_url_click = false;
        outcome
    }

    pub(super) fn handle_raw_events(&mut self, events: Vec<RawInputEvent>) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        for event in events {
            let ready = self.bus_terminal_ready();
            if self
                .bus
                .as_mut()
                .is_some_and(|bus| bus.input(&event, ready, &mut outcome))
            {
                continue;
            }
            if let Some(update) = host_theme_update(&event) {
                push_host_theme_update(&mut outcome.requests, update);
            }
            match event {
                RawInputEvent::Key(key) => self.handle_key(key, &mut outcome),
                RawInputEvent::Text(text) => {
                    let text = text.into_string();
                    self.prepare_committed_text(&mut outcome);
                    self.push_focused_pane_event(
                        ClientPaneInputEvent::TextCommit(text),
                        &mut outcome,
                    );
                }
                RawInputEvent::Paste(text) => {
                    self.prepare_committed_text(&mut outcome);
                    self.push_focused_pane_event(ClientPaneInputEvent::Paste(text), &mut outcome);
                }
                RawInputEvent::Mouse(mouse) => self.handle_mouse(mouse, &mut outcome),
                RawInputEvent::OuterFocusGained => {
                    self.outer_focused = Some(true);
                    outcome.query_host_appearance = true;
                    outcome.repaint |= self.config.redraw_on_focus_gained;
                    if let Some(surface) = self.pane_surface.clone() {
                        outcome.repaint |= self.acknowledge_active_surface_agents(&surface);
                    }
                    outcome
                        .requests
                        .push(ClientMessage::ClientShellFocus { focused: true });
                }
                RawInputEvent::OuterFocusLost => {
                    self.outer_focused = Some(false);
                    self.release_input_leases(&mut outcome);
                    outcome
                        .requests
                        .push(ClientMessage::ClientShellFocus { focused: false });
                }
                RawInputEvent::HostColorSchemeChanged(appearance) => {
                    self.host_appearance = Some(appearance);
                    self.host_appearance_explicit = true;
                    outcome.query_host_theme = true;
                    if self.config.theme_runtime.auto_switch {
                        self.config.palette = crate::app::client_palette_for_appearance(
                            &self.config.theme_runtime,
                            appearance,
                        );
                        outcome.repaint = true;
                    }
                }
                RawInputEvent::HostDefaultColor {
                    kind: crate::terminal_theme::DefaultColorKind::Background,
                    color,
                } if !self.host_appearance_explicit => {
                    let appearance = color.inferred_appearance();
                    self.host_appearance = Some(appearance);
                    if self.config.theme_runtime.auto_switch {
                        self.config.palette = crate::app::client_palette_for_appearance(
                            &self.config.theme_runtime,
                            appearance,
                        );
                        outcome.repaint = true;
                    }
                }
                RawInputEvent::HostDefaultColor { .. }
                | RawInputEvent::HostPaletteColors { .. }
                | RawInputEvent::HostCellSizeReport { .. }
                | RawInputEvent::Unsupported => {}
            }
        }
        outcome
    }

    pub(super) fn handle_key(
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

    fn release_input_leases(&mut self, outcome: &mut ClientShellInput) {
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
            super::push_target_event(
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

    pub(super) fn focused_pane_id(&self) -> Option<String> {
        self.snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.focused_pane_id.clone())
    }

    fn push_pane_key(
        &self,
        target: ClientInputTarget,
        key: crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) {
        if let Some(event) = ClientPaneInputEvent::from_terminal_key(key) {
            super::push_target_event(target, event, outcome);
        }
    }

    fn push_focused_pane_event(&self, event: ClientPaneInputEvent, outcome: &mut ClientShellInput) {
        if let Some(pane_id) = self.focused_pane_id() {
            super::push_target_event(ClientInputTarget::Pane(pane_id), event, outcome);
        }
    }
}
