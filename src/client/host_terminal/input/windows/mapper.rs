//! Record mapping, surrogate state, mouse buttons, and native key provenance.
use super::keymap::{
    ctrl_key_code, resolve_ctrl_oem_char, windows_input_trace_enabled, windows_key_modifiers,
    windows_virtual_key_to_char_code, windows_virtual_key_to_key_code,
};
use super::records::{PlatformInputItem, WindowsInputRecord, WindowsMouseRecord};
use super::win32_input_mode::{WindowsWin32InputModeFramer, WindowsWin32InputModeItem};
use crate::input::WindowsKeyRecord;

#[derive(Default)]
pub(super) struct WindowsInputMapper {
    pending_high_surrogate: Option<u16>,
    pending_paste_high_surrogate: Option<u16>,
    mouse_buttons: WindowsMouseButtons,
    win32_input: WindowsWin32InputModeFramer,
}

#[derive(Clone, Copy, Debug, Default)]
struct WindowsMouseButtons {
    left: bool,
    right: bool,
    middle: bool,
}

impl WindowsInputMapper {
    pub(super) fn idle(&mut self) -> Vec<PlatformInputItem> {
        self.win32_input
            .flush_timeout()
            .into_iter()
            .map(PlatformInputItem::Bytes)
            .collect()
    }

    pub(super) fn translate(&mut self, record: WindowsInputRecord) -> Vec<PlatformInputItem> {
        match record {
            WindowsInputRecord::Key(key) => self.translate_key(key),
            WindowsInputRecord::Mouse(mouse) => {
                let items = self
                    .translate_mouse(mouse)
                    .map(PlatformInputItem::Semantic)
                    .into_iter()
                    .collect();
                self.with_pending_win32_flush(items)
            }
            WindowsInputRecord::Focus(focused) => {
                self.with_pending_win32_flush(vec![PlatformInputItem::Semantic(if focused {
                    crate::protocol::ClientInputEvent::FocusGained
                } else {
                    crate::protocol::ClientInputEvent::FocusLost
                })])
            }
        }
    }

    fn translate_key(&mut self, key: WindowsKeyRecord) -> Vec<PlatformInputItem> {
        if windows_input_trace_enabled() {
            tracing::info!(
                key_down = key.key_down,
                repeat_count = key.repeat_count,
                virtual_key_code = key.virtual_key_code,
                virtual_scan_code = key.virtual_scan_code,
                unicode = key.unicode,
                control_key_state = key.control_key_state,
                "windows input trace: console key record"
            );
        }

        if !self.key_record_can_emit_event(key) {
            return Vec::new();
        }

        if self.key_record_is_raw_escape(key) {
            return self.translate_win32_input_mode_bytes(&[0x1b]);
        }

        let repeat_count = key.repeat_count.max(1);
        if key.virtual_key_code == 0 {
            let mut items = Vec::new();
            let mut flush_pending_win32_before_items = false;
            for repeat_idx in 0..repeat_count {
                let kind = Self::semantic_key_kind(key, false, repeat_idx);
                if let Some((bytes, event)) = self.synthetic_modified_key_event(key, kind) {
                    flush_pending_win32_before_items = true;
                    items.push(PlatformInputItem::PasteAwareKey {
                        win32_paste_bytes: bytes.clone(),
                        bytes,
                        events: vec![event],
                    });
                } else if let Some(bytes) = self.synthetic_utf16_unit_to_bytes(key.unicode) {
                    items.extend(self.translate_win32_input_mode_bytes(&bytes));
                }
            }
            return if flush_pending_win32_before_items {
                self.with_pending_win32_flush(items)
            } else {
                items
            };
        }

        let events = self.translate_semantic_key_events(key, resolve_ctrl_oem_char(key));
        let items = if let Some(bytes) = self.paste_payload_bytes_for_key(key) {
            vec![PlatformInputItem::PasteAwareKey {
                win32_paste_bytes: bytes.clone(),
                bytes,
                events,
            }]
        } else {
            events
                .into_iter()
                .map(PlatformInputItem::Semantic)
                .collect()
        };
        self.with_pending_win32_flush(items)
    }

    fn key_record_is_raw_escape(&self, key: WindowsKeyRecord) -> bool {
        let modifiers = windows_key_modifiers(key.control_key_state);
        if !key.key_down
            || key.repeat_count.max(1) != 1
            || modifiers.contains(crossterm::event::KeyModifiers::ALT)
        {
            return false;
        }

        // Physical Escape carries a scan code. Scan-code-zero Escape can
        // introduce raw VT reports and must stay in the framer.
        let bare_escape = modifiers.is_empty()
            && ((key.virtual_key_code == 0x1b && key.virtual_scan_code == 0)
                || (key.virtual_key_code == 0 && key.unicode == 0x1b));
        let ctrl_bracket = key.virtual_key_code == 0xdb
            && key.unicode == 0x1b
            && modifiers == crossterm::event::KeyModifiers::CONTROL;

        bare_escape || ctrl_bracket
    }

    fn translate_win32_input_mode_bytes(&mut self, bytes: &[u8]) -> Vec<PlatformInputItem> {
        let mut items = Vec::new();
        for item in self.win32_input.push(bytes) {
            match item {
                WindowsWin32InputModeItem::Bytes(bytes) => {
                    items.push(PlatformInputItem::Bytes(bytes))
                }
                WindowsWin32InputModeItem::Key { bytes, record } => {
                    let win32_paste_bytes =
                        self.paste_payload_bytes_for_key(record).unwrap_or_default();
                    if let Some(raw_bytes) = self.win32_input_mode_key_record_raw_bytes(record) {
                        items.push(PlatformInputItem::PasteAwareBytes {
                            paste_bytes: bytes,
                            raw_bytes,
                            win32_paste_bytes,
                        });
                    } else {
                        let oem_char = resolve_ctrl_oem_char(record);
                        items.push(PlatformInputItem::PasteAwareKey {
                            bytes,
                            win32_paste_bytes,
                            events: self.translate_semantic_key_events(record, oem_char),
                        })
                    }
                }
            }
        }
        items
    }

    fn win32_input_mode_key_record_raw_bytes(
        &mut self,
        record: WindowsKeyRecord,
    ) -> Option<Vec<u8>> {
        if self.key_record_is_raw_escape(record) {
            return Some(vec![0x1b]);
        }

        if record.virtual_scan_code != 0 {
            return None;
        }

        if !record.key_down
            || record.repeat_count.max(1) != 1
            || windows_key_modifiers(record.control_key_state).bits() != 0
        {
            return None;
        }

        if record.virtual_key_code != 0 && (record.unicode < 0x20 || record.unicode == 0x7f) {
            return None;
        }

        self.synthetic_utf16_unit_to_bytes(record.unicode)
    }

    fn with_pending_win32_flush(
        &mut self,
        mut items: Vec<PlatformInputItem>,
    ) -> Vec<PlatformInputItem> {
        let mut pending = self.idle();
        pending.append(&mut items);
        pending
    }

    fn synthetic_modified_key_event(
        &mut self,
        key: WindowsKeyRecord,
        kind: crate::protocol::ClientKeyKind,
    ) -> Option<(Vec<u8>, crate::protocol::ClientInputEvent)> {
        use crate::protocol::ClientKeyCode;

        let modifiers = windows_key_modifiers(key.control_key_state);
        if !modifiers.contains(crossterm::event::KeyModifiers::SHIFT) {
            return None;
        }

        let code = match key.unicode {
            0x0009 => ClientKeyCode::BackTab,
            0x000d => ClientKeyCode::Enter,
            _ => return None,
        };
        self.pending_high_surrogate = None;
        self.pending_paste_high_surrogate = None;
        Some((
            vec![key.unicode as u8],
            crate::protocol::ClientInputEvent::Key {
                code,
                modifiers: modifiers.bits(),
                kind,
                repeat_count: 1,

                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
        ))
    }

    pub(super) fn translate_semantic_key_events(
        &mut self,
        key: WindowsKeyRecord,
        oem_char: Option<char>,
    ) -> Vec<crate::protocol::ClientInputEvent> {
        if !self.key_record_can_emit_event(key) {
            return Vec::new();
        }
        if Self::key_record_is_modifier_only(key) {
            return Vec::new();
        }

        if key.virtual_key_code == 0 {
            if let Some((_bytes, event)) =
                self.synthetic_modified_key_event(key, crate::protocol::ClientKeyKind::Press)
            {
                return vec![event];
            }
        }

        let is_alt_code = Self::is_alt_code(key);
        let carries_native_record = key.repeat_count != 0
            && key.virtual_key_code != 0
            && key.virtual_key_code != 0x03
            && !matches!(key.virtual_key_code, 0xe5 | 0xe7)
            && !is_alt_code
            && !(0xd800..=0xdfff).contains(&key.unicode);
        if carries_native_record {
            let kind = Self::semantic_key_kind(key, is_alt_code, 0);
            return self
                .translate_semantic_key_event(key, kind, oem_char)
                .map(|event| match event {
                    crate::protocol::ClientInputEvent::Key {
                        code,
                        modifiers,
                        kind,
                        generated_text,
                        ..
                    } => {
                        // Windows Unicode is the authoritative host-layout result. Plain and
                        // AltGr characters already encode from `code`; Shift must remain on the
                        // physical event, so carry its produced text separately.
                        let shifted_text = key.key_down
                            && modifiers == crossterm::event::KeyModifiers::SHIFT.bits();
                        crate::protocol::ClientInputEvent::Key {
                            code,
                            modifiers,
                            kind,
                            repeat_count: key.repeat_count.max(1),
                            generated_text: if shifted_text { generated_text } else { None },
                            source: crate::protocol::ClientKeySource::WindowsConsole {
                                record: key,
                            },
                        }
                    }
                    event => event,
                })
                .into_iter()
                .collect();
        }

        (0..key.repeat_count.max(1))
            .filter_map(|repeat_idx| {
                self.translate_semantic_key_event(
                    key,
                    Self::semantic_key_kind(key, is_alt_code, repeat_idx),
                    oem_char,
                )
            })
            .collect()
    }

    fn key_record_can_emit_event(&self, key: WindowsKeyRecord) -> bool {
        key.key_down || Self::is_alt_code(key) || key.virtual_key_code != 0
    }

    fn key_record_is_modifier_only(key: WindowsKeyRecord) -> bool {
        matches!(
            key.virtual_key_code,
            0x10 | 0x11 | 0x12 | 0xa0 | 0xa1 | 0xa2 | 0xa3 | 0xa4 | 0xa5
        ) && key.unicode == 0
    }

    fn is_alt_code(key: WindowsKeyRecord) -> bool {
        const VK_MENU: u16 = 0x12;
        key.virtual_key_code == VK_MENU && !key.key_down && key.unicode != 0
    }

    fn semantic_key_kind(
        key: WindowsKeyRecord,
        is_alt_code: bool,
        repeat_idx: u16,
    ) -> crate::protocol::ClientKeyKind {
        if is_alt_code {
            crate::protocol::ClientKeyKind::Press
        } else if !key.key_down {
            crate::protocol::ClientKeyKind::Release
        } else if repeat_idx > 0 {
            crate::protocol::ClientKeyKind::Repeat
        } else {
            crate::protocol::ClientKeyKind::Press
        }
    }

    fn translate_semantic_key_event(
        &mut self,
        key: WindowsKeyRecord,
        kind: crate::protocol::ClientKeyKind,
        oem_char: Option<char>,
    ) -> Option<crate::protocol::ClientInputEvent> {
        let modifiers = windows_key_modifiers(key.control_key_state);
        if key.virtual_key_code == 0 {
            let codepoint = self.utf16_unit_to_char(key.unicode)?;
            if !codepoint.is_control() {
                return Some(crate::protocol::ClientInputEvent::TextCommit(
                    codepoint.to_string(),
                ));
            }
        }
        if modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
            && key.unicode == 0x000a
            && (key.virtual_key_code == 0x4a || key.virtual_scan_code == 0x24)
        {
            self.pending_high_surrogate = None;
            return Some(crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Char('j'),
                modifiers: modifiers.bits(),
                kind,
                repeat_count: 1,

                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            });
        }

        let code = if let Some(code) =
            windows_virtual_key_to_key_code(key.virtual_key_code, modifiers)
        {
            self.pending_high_surrogate = None;
            Some(code)
        } else {
            if key.unicode == 0 {
                self.pending_high_surrogate = None;
            }
            if modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                && !(0x30..=0x39).contains(&key.virtual_key_code)
            {
                if let Some(code) = ctrl_key_code(key.virtual_key_code, key.unicode, oem_char) {
                    self.pending_high_surrogate = None;
                    return Some(crate::protocol::ClientInputEvent::Key {
                        code,
                        modifiers: modifiers.bits(),
                        kind,
                        repeat_count: 1,

                        generated_text: None,
                        source: crate::protocol::ClientKeySource::Synthesized,
                    });
                }
            }
            self.utf16_unit_to_char(key.unicode)
                .filter(|ch| !ch.is_control())
                .map(crate::protocol::ClientKeyCode::Char)
                .or_else(|| {
                    windows_virtual_key_to_char_code(key.virtual_key_code, key.unicode, modifiers)
                })
        };

        code.map(|code| {
            let generated_text = matches!(code, crate::protocol::ClientKeyCode::Char(_))
                .then(|| char::from_u32(key.unicode as u32))
                .flatten()
                .filter(|ch| !ch.is_control())
                .map(|ch| ch.to_string());
            crate::protocol::ClientInputEvent::Key {
                code,
                modifiers: modifiers.bits(),
                kind,
                repeat_count: 1,
                generated_text,
                source: crate::protocol::ClientKeySource::Synthesized,
            }
        })
    }

    fn utf16_unit_to_char(&mut self, unit: u16) -> Option<char> {
        Self::utf16_unit_to_char_with_pending(&mut self.pending_high_surrogate, unit)
    }

    fn utf16_unit_to_char_with_pending(
        pending_high_surrogate: &mut Option<u16>,
        unit: u16,
    ) -> Option<char> {
        if unit == 0 {
            return None;
        }

        if (0xd800..=0xdbff).contains(&unit) {
            *pending_high_surrogate = Some(unit);
            return None;
        }

        let ch = if (0xdc00..=0xdfff).contains(&unit) {
            let high = pending_high_surrogate.take()?;
            let codepoint = 0x10000 + (((high as u32 - 0xd800) << 10) | (unit as u32 - 0xdc00));
            char::from_u32(codepoint)?
        } else {
            *pending_high_surrogate = None;
            char::from_u32(unit as u32)?
        };

        Some(ch)
    }

    fn paste_payload_bytes_for_key(&mut self, key: WindowsKeyRecord) -> Option<Vec<u8>> {
        if key.unicode == 0 {
            return None;
        }
        if !key.key_down {
            return Some(Vec::new());
        }

        let ch = Self::utf16_unit_to_char_with_pending(
            &mut self.pending_paste_high_surrogate,
            key.unicode,
        )?;
        let mut buf = [0; 4];
        Some(
            ch.encode_utf8(&mut buf)
                .as_bytes()
                .repeat(key.repeat_count.max(1) as usize),
        )
    }

    fn synthetic_utf16_unit_to_bytes(&mut self, unit: u16) -> Option<Vec<u8>> {
        let ch = self.utf16_unit_to_char(unit)?;
        let mut buf = [0; 4];
        Some(ch.encode_utf8(&mut buf).as_bytes().to_vec())
    }

    fn translate_mouse(
        &mut self,
        mouse: WindowsMouseRecord,
    ) -> Option<crate::protocol::ClientInputEvent> {
        use crossterm::event::{MouseButton, MouseEventKind};

        const FROM_LEFT_1ST_BUTTON_PRESSED: u32 = 0x0001;
        const RIGHTMOST_BUTTON_PRESSED: u32 = 0x0002;
        const FROM_LEFT_2ND_BUTTON_PRESSED: u32 = 0x0004;
        const MOUSE_MOVED: u32 = 0x0001;
        const MOUSE_WHEELED: u32 = 0x0004;
        const MOUSE_HWHEELED: u32 = 0x0008;

        let buttons = WindowsMouseButtons {
            left: mouse.button_state & FROM_LEFT_1ST_BUTTON_PRESSED != 0,
            right: mouse.button_state & RIGHTMOST_BUTTON_PRESSED != 0,
            middle: mouse.button_state & FROM_LEFT_2ND_BUTTON_PRESSED != 0,
        };

        let kind = if mouse.event_flags & MOUSE_WHEELED != 0 {
            if (mouse.button_state as i32) < 0 {
                MouseEventKind::ScrollDown
            } else {
                MouseEventKind::ScrollUp
            }
        } else if mouse.event_flags & MOUSE_HWHEELED != 0 {
            if (mouse.button_state as i32) < 0 {
                MouseEventKind::ScrollLeft
            } else {
                MouseEventKind::ScrollRight
            }
        } else if mouse.event_flags & MOUSE_MOVED != 0 {
            if buttons.left {
                MouseEventKind::Drag(MouseButton::Left)
            } else if buttons.right {
                MouseEventKind::Drag(MouseButton::Right)
            } else if buttons.middle {
                MouseEventKind::Drag(MouseButton::Middle)
            } else {
                MouseEventKind::Moved
            }
        } else if buttons.left && !self.mouse_buttons.left {
            MouseEventKind::Down(MouseButton::Left)
        } else if buttons.right && !self.mouse_buttons.right {
            MouseEventKind::Down(MouseButton::Right)
        } else if buttons.middle && !self.mouse_buttons.middle {
            MouseEventKind::Down(MouseButton::Middle)
        } else if !buttons.left && self.mouse_buttons.left {
            MouseEventKind::Up(MouseButton::Left)
        } else if !buttons.right && self.mouse_buttons.right {
            MouseEventKind::Up(MouseButton::Right)
        } else if !buttons.middle && self.mouse_buttons.middle {
            MouseEventKind::Up(MouseButton::Middle)
        } else {
            self.mouse_buttons = buttons;
            return None;
        };
        self.mouse_buttons = buttons;

        Some(crate::protocol::ClientInputEvent::Mouse {
            kind: crate::protocol::ClientMouseKind::from_crossterm(kind),
            column: mouse.x,
            row: mouse.y,
            modifiers: windows_key_modifiers(mouse.control_key_state).bits(),
        })
    }
}
