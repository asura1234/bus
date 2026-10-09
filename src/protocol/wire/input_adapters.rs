#[cfg(any(windows, test))]
use super::input::{ClientInputEvent, ClientKeySource};
use super::input::{
    ClientKeyCode, ClientKeyKind, ClientMouseButton, ClientMouseKind, ClientMousePosition,
    ClientPaneInputEvent,
};

impl ClientKeyKind {
    pub(crate) fn from_crossterm(kind: crossterm::event::KeyEventKind) -> Self {
        match kind {
            crossterm::event::KeyEventKind::Press => Self::Press,
            crossterm::event::KeyEventKind::Repeat => Self::Repeat,
            crossterm::event::KeyEventKind::Release => Self::Release,
        }
    }

    pub(crate) fn to_crossterm(self) -> crossterm::event::KeyEventKind {
        match self {
            Self::Press => crossterm::event::KeyEventKind::Press,
            Self::Repeat => crossterm::event::KeyEventKind::Repeat,
            Self::Release => crossterm::event::KeyEventKind::Release,
        }
    }
}

impl ClientKeyCode {
    pub(crate) fn from_crossterm(code: crossterm::event::KeyCode) -> Option<Self> {
        use crossterm::event::KeyCode;
        Some(match code {
            KeyCode::Backspace => Self::Backspace,
            KeyCode::Enter => Self::Enter,
            KeyCode::Left => Self::Left,
            KeyCode::Right => Self::Right,
            KeyCode::Up => Self::Up,
            KeyCode::Down => Self::Down,
            KeyCode::Home => Self::Home,
            KeyCode::End => Self::End,
            KeyCode::PageUp => Self::PageUp,
            KeyCode::PageDown => Self::PageDown,
            KeyCode::Tab => Self::Tab,
            KeyCode::BackTab => Self::BackTab,
            KeyCode::Delete => Self::Delete,
            KeyCode::Insert => Self::Insert,
            KeyCode::Esc => Self::Esc,
            KeyCode::Char(ch) => Self::Char(ch),
            KeyCode::F(n) => Self::F(n),
            KeyCode::Null => Self::Null,
            _ => return None,
        })
    }

    pub(crate) fn to_crossterm(&self) -> crossterm::event::KeyCode {
        use crossterm::event::KeyCode;
        match self {
            Self::Backspace => KeyCode::Backspace,
            Self::Enter => KeyCode::Enter,
            Self::Left => KeyCode::Left,
            Self::Right => KeyCode::Right,
            Self::Up => KeyCode::Up,
            Self::Down => KeyCode::Down,
            Self::Home => KeyCode::Home,
            Self::End => KeyCode::End,
            Self::PageUp => KeyCode::PageUp,
            Self::PageDown => KeyCode::PageDown,
            Self::Tab => KeyCode::Tab,
            Self::BackTab => KeyCode::BackTab,
            Self::Delete => KeyCode::Delete,
            Self::Insert => KeyCode::Insert,
            Self::Esc => KeyCode::Esc,
            Self::Char(ch) => KeyCode::Char(*ch),
            Self::F(n) => KeyCode::F(*n),
            Self::Null => KeyCode::Null,
        }
    }
}

impl ClientMouseButton {
    pub(crate) fn from_crossterm(button: crossterm::event::MouseButton) -> Self {
        match button {
            crossterm::event::MouseButton::Left => Self::Left,
            crossterm::event::MouseButton::Right => Self::Right,
            crossterm::event::MouseButton::Middle => Self::Middle,
        }
    }

    pub(crate) fn to_crossterm(self) -> crossterm::event::MouseButton {
        match self {
            Self::Left => crossterm::event::MouseButton::Left,
            Self::Right => crossterm::event::MouseButton::Right,
            Self::Middle => crossterm::event::MouseButton::Middle,
        }
    }
}

impl ClientMouseKind {
    pub(crate) fn from_crossterm(kind: crossterm::event::MouseEventKind) -> Self {
        use crossterm::event::MouseEventKind;
        match kind {
            MouseEventKind::Down(button) => Self::Down(ClientMouseButton::from_crossterm(button)),
            MouseEventKind::Up(button) => Self::Up(ClientMouseButton::from_crossterm(button)),
            MouseEventKind::Drag(button) => Self::Drag(ClientMouseButton::from_crossterm(button)),
            MouseEventKind::Moved => Self::Moved,
            MouseEventKind::ScrollUp => Self::ScrollUp,
            MouseEventKind::ScrollDown => Self::ScrollDown,
            MouseEventKind::ScrollLeft => Self::ScrollLeft,
            MouseEventKind::ScrollRight => Self::ScrollRight,
        }
    }

    pub(crate) fn to_crossterm(self) -> crossterm::event::MouseEventKind {
        use crossterm::event::MouseEventKind;
        match self {
            Self::Down(button) => MouseEventKind::Down(button.to_crossterm()),
            Self::Up(button) => MouseEventKind::Up(button.to_crossterm()),
            Self::Drag(button) => MouseEventKind::Drag(button.to_crossterm()),
            Self::Moved => MouseEventKind::Moved,
            Self::ScrollUp => MouseEventKind::ScrollUp,
            Self::ScrollDown => MouseEventKind::ScrollDown,
            Self::ScrollLeft => MouseEventKind::ScrollLeft,
            Self::ScrollRight => MouseEventKind::ScrollRight,
        }
    }
}

impl ClientPaneInputEvent {
    pub(crate) fn from_terminal_key(key: crate::protocol::keys::TerminalKey) -> Option<Self> {
        let tracks_release = key.generated_text.is_none() || key.has_physical_identity();
        let physical_key_id = key.physical_key_id();
        let windows_record = key.windows_record();
        Some(Self::Key {
            code: ClientKeyCode::from_crossterm(key.code)?,
            modifiers: key.modifiers.bits(),
            kind: ClientKeyKind::from_crossterm(key.kind),
            repeat_count: key.repeat_count,
            shifted_codepoint: key.shifted_codepoint,
            generated_text: key.generated_text,
            tracks_release,
            physical_key_id,
            windows_record,
        })
    }

    pub(crate) fn to_raw_input_event(&self) -> crate::protocol::keys::host::RawInputEvent {
        self.to_raw_input_event_with_windows_source(cfg!(any(windows, test)))
    }

    pub(super) fn to_raw_input_event_with_windows_source(
        &self,
        attach_windows_source: bool,
    ) -> crate::protocol::keys::host::RawInputEvent {
        match self {
            Self::Key {
                code,
                modifiers,
                kind,
                repeat_count,
                shifted_codepoint,
                generated_text,
                tracks_release,
                windows_record,
                ..
            } => {
                let mut key = crate::protocol::keys::TerminalKey::new(
                    code.to_crossterm(),
                    crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
                )
                .with_kind(kind.to_crossterm())
                .with_repeat_count(*repeat_count)
                .with_generated_text(generated_text.clone())
                .with_physical_identity_hint(*tracks_release && generated_text.is_some())
                .with_windows_composition_hint(*windows_record);
                if let Some(shifted_codepoint) = shifted_codepoint {
                    key = key.with_shifted_codepoint(*shifted_codepoint);
                }
                #[cfg(any(windows, test))]
                if attach_windows_source {
                    if let Some(record) = windows_record {
                        key = key.with_windows_record(*record);
                    }
                }
                #[cfg(not(any(windows, test)))]
                let _ = (attach_windows_source, windows_record);
                crate::protocol::keys::host::RawInputEvent::Key(key)
            }
            Self::TextCommit(text) => crate::protocol::keys::host::RawInputEvent::Text(
                crate::protocol::keys::TextCommit::new(text.clone()),
            ),
            Self::Mouse {
                kind,
                position,
                modifiers,
                ..
            } => {
                let (column, row) = match position {
                    ClientMousePosition::Cell { column, row }
                    | ClientMousePosition::Pixels { column, row, .. } => (*column, *row),
                };
                crate::protocol::keys::host::RawInputEvent::Mouse(crossterm::event::MouseEvent {
                    kind: kind.to_crossterm(),
                    column,
                    row,
                    modifiers: crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
                })
            }
            Self::Paste(text) => crate::protocol::keys::host::RawInputEvent::Paste(text.clone()),
        }
    }
}

#[cfg(any(windows, test))]
impl ClientInputEvent {
    pub(crate) fn from_crossterm(event: crossterm::event::Event) -> Option<Self> {
        match event {
            crossterm::event::Event::Key(key) => Some(Self::Key {
                code: ClientKeyCode::from_crossterm(key.code)?,
                modifiers: key.modifiers.bits(),
                kind: ClientKeyKind::from_crossterm(key.kind),
                repeat_count: 1,
                generated_text: None,
                source: ClientKeySource::Synthesized,
            }),
            crossterm::event::Event::Mouse(mouse) => Some(Self::Mouse {
                kind: ClientMouseKind::from_crossterm(mouse.kind),
                column: mouse.column,
                row: mouse.row,
                modifiers: mouse.modifiers.bits(),
            }),
            crossterm::event::Event::Paste(text) => Some(Self::Paste { text }),
            crossterm::event::Event::FocusGained => Some(Self::FocusGained),
            crossterm::event::Event::FocusLost => Some(Self::FocusLost),
            crossterm::event::Event::Resize(_, _) => None,
        }
    }

    pub(crate) fn to_raw_input_event(&self) -> crate::protocol::keys::host::RawInputEvent {
        match self {
            Self::Key {
                code,
                modifiers,
                kind,
                repeat_count,
                generated_text,
                source,
            } => {
                let mut key = crate::protocol::keys::TerminalKey::new(
                    code.to_crossterm(),
                    crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
                )
                .with_generated_text(generated_text.clone());
                key = match source {
                    ClientKeySource::Synthesized => key,
                    ClientKeySource::Vt { bytes } => key.with_vt_bytes(bytes.clone()),
                    ClientKeySource::WindowsConsole { record } => key.with_windows_record(*record),
                };
                key = key
                    .with_repeat_count(*repeat_count)
                    .with_kind(kind.to_crossterm());
                crate::protocol::keys::host::RawInputEvent::Key(key)
            }
            Self::TextCommit(text) => crate::protocol::keys::host::RawInputEvent::Text(
                crate::protocol::keys::TextCommit::new(text.clone()),
            ),
            Self::Mouse {
                kind,
                column,
                row,
                modifiers,
            } => crate::protocol::keys::host::RawInputEvent::Mouse(crossterm::event::MouseEvent {
                kind: kind.to_crossterm(),
                column: *column,
                row: *row,
                modifiers: crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
            }),
            Self::Paste { text } => crate::protocol::keys::host::RawInputEvent::Paste(text.clone()),
            Self::FocusGained => crate::protocol::keys::host::RawInputEvent::OuterFocusGained,
            Self::FocusLost => crate::protocol::keys::host::RawInputEvent::OuterFocusLost,
        }
    }
}
