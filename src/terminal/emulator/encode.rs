use super::controls::input::{
    ghostty_key_event_from_terminal_key, ghostty_mouse_encoder_for_terminal,
    ghostty_mouse_event_from_button_kind, ghostty_mouse_event_from_motion_kind,
    ghostty_mouse_event_from_wheel_kind, ghostty_mouse_position_for_terminal,
    ghostty_prefers_herdr_text_encoding,
};
use super::{GhosttyPaneTerminal, PaneTerminal, MODE_MOUSE_ANY_MOTION};

impl PaneTerminal {
    pub fn encode_terminal_key(
        &self,
        key: crate::protocol::keys::TerminalKey,
        protocol: crate::protocol::keys::KeyboardProtocol,
    ) -> Vec<u8> {
        self.ghostty.encode_terminal_key(key, protocol)
    }

    pub(crate) fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::protocol::keys::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.ghostty.encode_mouse_button(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::protocol::keys::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.ghostty.encode_mouse_motion(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::protocol::keys::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.ghostty.encode_mouse_wheel(kind, position, modifiers)
    }
}

impl GhosttyPaneTerminal {
    pub fn encode_terminal_key(
        &self,
        key: crate::protocol::keys::TerminalKey,
        protocol: crate::protocol::keys::KeyboardProtocol,
    ) -> Vec<u8> {
        #[cfg(windows)]
        if self.core.lock().is_ok_and(|core| {
            core.terminal
                .kitty_keyboard_flags()
                .is_ok_and(|flags| flags == 0)
                && !core.kitty_keyboard.modify_other_keys_enabled()
        }) {
            if let Some(bytes) = crate::protocol::keys::encode_windows_conpty_fallback(&key) {
                return bytes;
            }
        }

        let repeat_count = key.repeat_count;
        let first = key.with_repeat_count(1);
        let mut bytes = self.encode_terminal_key_once(first.clone(), protocol);
        if repeat_count > 1 && first.kind != crossterm::event::KeyEventKind::Release {
            let repeated = first.with_kind(crossterm::event::KeyEventKind::Repeat);
            let repeated_bytes = self.encode_terminal_key_once(repeated, protocol);
            for _ in 1..repeat_count {
                bytes.extend_from_slice(&repeated_bytes);
            }
        }
        bytes
    }

    fn encode_terminal_key_once(
        &self,
        key: crate::protocol::keys::TerminalKey,
        protocol: crate::protocol::keys::KeyboardProtocol,
    ) -> Vec<u8> {
        if matches!(protocol, crate::protocol::keys::KeyboardProtocol::Legacy)
            && key.code == crossterm::event::KeyCode::Tab
            && key.modifiers == crossterm::event::KeyModifiers::CONTROL
        {
            return crate::protocol::keys::encode_terminal_key(key, protocol);
        }

        if ghostty_prefers_herdr_text_encoding(&key) {
            return crate::protocol::keys::encode_terminal_key(key, protocol);
        }

        let Some(event) = ghostty_key_event_from_terminal_key(&key) else {
            return crate::protocol::keys::encode_terminal_key(key, protocol);
        };

        let Ok(mut encoder) = self.key_encoder.lock() else {
            return crate::protocol::keys::encode_terminal_key(key, protocol);
        };
        match encoder.encode(&event) {
            Ok(bytes)
                if !bytes.is_empty()
                    && encoded_key_preserves_event_kind(&bytes, &key, protocol) =>
            {
                bytes
            }
            Ok(_) | Err(_) => crate::protocol::keys::encode_terminal_key(key, protocol),
        }
    }

    pub(crate) fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::protocol::keys::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.encode_mouse_event(
            ghostty_mouse_event_from_button_kind(kind, 0, 0, modifiers)?,
            position,
            false,
        )
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::protocol::keys::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.encode_mouse_event(
            ghostty_mouse_event_from_motion_kind(kind, 0, 0, modifiers)?,
            position,
            true,
        )
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::protocol::keys::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.encode_mouse_event(
            ghostty_mouse_event_from_wheel_kind(kind, 0, 0, modifiers)?,
            position,
            false,
        )
    }

    fn encode_mouse_event(
        &self,
        mut event: crate::terminal::vt::MouseEvent,
        position: crate::protocol::keys::mouse::Position,
        require_any_motion: bool,
    ) -> Option<Vec<u8>> {
        let core = self.core.lock().ok()?;
        if require_any_motion && !core.terminal.mode_get(MODE_MOUSE_ANY_MOTION).ok()? {
            return None;
        }
        let mut encoder = ghostty_mouse_encoder_for_terminal(&core.terminal, position)?;
        let (x, y) = ghostty_mouse_position_for_terminal(position)?;
        event.set_position(x, y);
        encoder
            .encode(&event)
            .ok()
            .filter(|bytes| !bytes.is_empty())
    }
}

fn encoded_key_preserves_event_kind(
    bytes: &[u8],
    key: &crate::protocol::keys::TerminalKey,
    protocol: crate::protocol::keys::KeyboardProtocol,
) -> bool {
    if !protocol.reports_event_types() || key.kind == crossterm::event::KeyEventKind::Press {
        return true;
    }

    std::str::from_utf8(bytes)
        .ok()
        .and_then(crate::protocol::keys::parse_terminal_key_sequence)
        .is_some_and(|parsed| {
            parsed.code == key.code && parsed.modifiers == key.modifiers && parsed.kind == key.kind
        })
}
