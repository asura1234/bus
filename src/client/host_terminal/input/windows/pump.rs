//! Bracketed paste and physical Escape framing into client input events.
use super::super::windows_client_input_event_from_raw;
use super::records::PlatformInputItem;

pub(super) struct WindowsInputPump {
    framer: crate::raw_input::RawInputFramer,
    paste_from_win32_key_records: bool,
    pending_physical_escape: Option<(crate::protocol::ClientInputEvent, bool)>,
}

impl Default for WindowsInputPump {
    fn default() -> Self {
        Self {
            framer: crate::raw_input::RawInputFramer::for_host_input(),
            paste_from_win32_key_records: false,
            pending_physical_escape: None,
        }
    }
}

impl WindowsInputPump {
    pub(super) fn process(
        &mut self,
        item: PlatformInputItem,
    ) -> Vec<crate::protocol::ClientInputEvent> {
        let mut events = Vec::new();
        if let Some((escape, open_bracket)) = self.pending_physical_escape.take() {
            let raw_bytes = item.raw_bytes();
            let continues_sgr = if open_bracket {
                raw_bytes.is_some_and(|bytes| bytes.starts_with(b"<"))
            } else {
                raw_bytes.is_some_and(|bytes| bytes.starts_with(b"[<"))
            };
            if continues_sgr {
                let prefix: &[u8] = if open_bracket { b"\x1b[" } else { b"\x1b" };
                let raw_events = self.framer.push(prefix);
                events.extend(self.process_raw_events(raw_events));
            } else if !open_bracket && raw_bytes == Some(b"[") {
                self.pending_physical_escape = Some((escape, true));
                return events;
            } else {
                events.push(escape);
                if open_bracket {
                    let raw_events = self.framer.push(b"[");
                    events.extend(self.process_raw_events(raw_events));
                }
            }
        }
        if let Some(escape) = item.physical_escape_press() {
            if !self.framer.has_pending_bracketed_paste() {
                let raw_events = self.framer.flush_timeout();
                events.extend(self.process_raw_events(raw_events));
                self.pending_physical_escape = Some((escape, false));
                return events;
            }
        }

        let mut next = match item {
            PlatformInputItem::Bytes(bytes) => {
                let raw_events = self.framer.push(&bytes);
                self.process_raw_events(raw_events)
            }
            PlatformInputItem::Semantic(event) => {
                let raw_events = self.framer.flush_timeout();
                let mut events = self.process_raw_events(raw_events);
                events.push(event);
                events
            }
            PlatformInputItem::PasteAwareBytes {
                paste_bytes,
                raw_bytes,
                win32_paste_bytes,
            } => {
                let pending_paste = self.framer.has_pending_bracketed_paste();
                let decode_win32_record = self.paste_from_win32_key_records || !pending_paste;
                let raw_events = if decode_win32_record {
                    if pending_paste {
                        self.framer.push(&win32_paste_bytes)
                    } else {
                        self.framer.push(&raw_bytes)
                    }
                } else {
                    self.framer.push(&paste_bytes)
                };
                if decode_win32_record && self.framer.has_pending_bracketed_paste() {
                    self.paste_from_win32_key_records = true;
                }
                self.process_raw_events(raw_events)
            }
            PlatformInputItem::PasteAwareKey {
                bytes,
                win32_paste_bytes,
                events,
            } => {
                if self.framer.has_pending_bracketed_paste() {
                    let bytes = if self.paste_from_win32_key_records {
                        &win32_paste_bytes
                    } else {
                        &bytes
                    };
                    let raw_events = self.framer.push(bytes);
                    self.process_raw_events(raw_events)
                } else {
                    let raw_events = self.framer.flush_timeout();
                    let mut output = self.process_raw_events(raw_events);
                    output.extend(events);
                    output
                }
            }
        };
        events.append(&mut next);
        events
    }

    pub(super) fn idle(&mut self) -> Vec<crate::protocol::ClientInputEvent> {
        let mut events = Vec::new();
        if let Some((escape, open_bracket)) = self.pending_physical_escape.take() {
            events.push(escape);
            if open_bracket {
                let raw_events = self.framer.push(b"[");
                events.extend(self.process_raw_events(raw_events));
            }
        }
        let raw_events = self.framer.flush_timeout();
        events.extend(self.process_raw_events(raw_events));
        events
    }

    fn process_raw_events(
        &mut self,
        mut events: Vec<crate::raw_input::RawInputEvent>,
    ) -> Vec<crate::protocol::ClientInputEvent> {
        for event in &mut events {
            if let crate::raw_input::RawInputEvent::Paste(text) = event {
                decode_windows_terminal_paste_enters(text);
                self.paste_from_win32_key_records = false;
            }
        }
        Self::raw_events_to_client_events(events)
    }

    fn raw_events_to_client_events(
        events: Vec<crate::raw_input::RawInputEvent>,
    ) -> Vec<crate::protocol::ClientInputEvent> {
        events
            .into_iter()
            .filter_map(windows_client_input_event_from_raw)
            .collect()
    }
}

impl PlatformInputItem {
    fn raw_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes)
            | Self::PasteAwareBytes {
                raw_bytes: bytes, ..
            } => Some(bytes),
            Self::Semantic(_) | Self::PasteAwareKey { .. } => None,
        }
    }

    fn physical_escape_press(&self) -> Option<crate::protocol::ClientInputEvent> {
        let event = match self {
            Self::Semantic(event) => event,
            Self::PasteAwareKey { events, .. } if events.len() == 1 => &events[0],
            _ => return None,
        };
        matches!(
            event,
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Esc,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Press,
                repeat_count: 1,
                source: crate::protocol::ClientKeySource::WindowsConsole { record },
                ..
            } if record.virtual_scan_code != 0
        )
        .then(|| event.clone())
    }
}

fn decode_windows_terminal_paste_enters(text: &mut String) {
    const ENTER_REPORT_PAIR: &str = "\x1b[13;28;13;1;0;1_\x1b[13;28;13;0;0;1_";

    // Windows Terminal can encode pasted newlines as an adjacent unmodified
    // Enter press/release pair. Keep every other report-shaped payload opaque.
    if text.contains(ENTER_REPORT_PAIR) {
        *text = text.replace(ENTER_REPORT_PAIR, "\r");
    }
}
