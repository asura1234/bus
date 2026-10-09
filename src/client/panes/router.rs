use crate::client::compositor::{
    push_target_event, ClientInputTarget, ClientShellInput, ClientShellState,
};
use crate::protocol::keys::host::RawInputEvent;
use crate::protocol::wire::ClientMessage;
use crate::protocol::wire::ClientPaneInputEvent;

fn host_theme_update(
    event: &RawInputEvent,
) -> Option<crate::protocol::wire::ClientHostThemeUpdate> {
    use crate::protocol::wire::{
        ClientHostAppearance, ClientHostDefaultColorKind, ClientHostThemeUpdate,
    };

    match event {
        RawInputEvent::HostDefaultColor { kind, color } => {
            Some(ClientHostThemeUpdate::DefaultColor {
                kind: match kind {
                    crate::utils::theme::color::DefaultColorKind::Foreground => {
                        ClientHostDefaultColorKind::Foreground
                    }
                    crate::utils::theme::color::DefaultColorKind::Background => {
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
                crate::utils::theme::color::HostAppearance::Dark => ClientHostAppearance::Dark,
                crate::utils::theme::color::HostAppearance::Light => ClientHostAppearance::Light,
            }))
        }
        _ => None,
    }
}

fn push_host_theme_update(
    requests: &mut Vec<ClientMessage>,
    update: crate::protocol::wire::ClientHostThemeUpdate,
) {
    if let crate::protocol::wire::ClientHostThemeUpdate::PaletteColors(colors) = &update {
        if let Some(ClientMessage::ClientShellHostTheme {
            update: crate::protocol::wire::ClientHostThemeUpdate::PaletteColors(pending),
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
        self.handle_raw_events(crate::protocol::keys::host::parse_raw_input_bytes_sync(
            data,
        ))
    }

    #[cfg(any(unix, test))]
    pub(crate) fn handle_pixel_mouse(
        &mut self,
        data: &[u8],
        geometry: crate::protocol::keys::mouse::HostGeometry,
    ) -> ClientShellInput {
        let Some((x, y)) = crate::protocol::keys::mouse::parse_report(data) else {
            return ClientShellInput::default();
        };
        let Some((column, row)) = geometry.cell(x, y) else {
            return ClientShellInput::default();
        };
        let Some(cell_report) = crate::protocol::keys::mouse::report_at_cell(data, column, row)
        else {
            return ClientShellInput::default();
        };
        let events = crate::protocol::keys::host::parse_raw_input_bytes_sync(&cell_report);
        if events.len() != 1 || !matches!(events[0], RawInputEvent::Mouse(_)) {
            return ClientShellInput::default();
        }
        self.host_mouse_pixels = Some(crate::protocol::keys::mouse::HostPixels { x, y, geometry });
        let outcome = self.handle_raw_events(events);
        self.host_mouse_pixels = None;
        outcome
    }

    #[cfg(windows)]
    pub(crate) fn handle_client_events(
        &mut self,
        events: &[crate::protocol::wire::ClientInputEvent],
    ) -> ClientShellInput {
        self.handle_raw_events(
            events
                .iter()
                .map(crate::protocol::wire::ClientInputEvent::to_raw_input_event)
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

    pub(in crate::client) fn handle_raw_events(
        &mut self,
        events: Vec<RawInputEvent>,
    ) -> ClientShellInput {
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
                        self.config.palette = crate::utils::theme::client_palette_for_appearance(
                            &self.config.theme_runtime,
                            appearance,
                        );
                        outcome.repaint = true;
                    }
                }
                RawInputEvent::HostDefaultColor {
                    kind: crate::utils::theme::color::DefaultColorKind::Background,
                    color,
                } if !self.host_appearance_explicit => {
                    let appearance = color.inferred_appearance();
                    self.host_appearance = Some(appearance);
                    if self.config.theme_runtime.auto_switch {
                        self.config.palette = crate::utils::theme::client_palette_for_appearance(
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

    pub(in crate::client) fn focused_pane_id(&self) -> Option<String> {
        self.snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.focused_pane_id.clone())
    }

    fn push_focused_pane_event(&self, event: ClientPaneInputEvent, outcome: &mut ClientShellInput) {
        if let Some(pane_id) = self.focused_pane_id() {
            push_target_event(ClientInputTarget::Pane(pane_id), event, outcome);
        }
    }
}
