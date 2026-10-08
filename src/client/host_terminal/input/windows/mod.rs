#[cfg(windows)]
use std::collections::VecDeque;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(windows)]
use std::sync::Arc;

#[cfg(windows)]
use tokio::sync::mpsc;

use super::windows_client_input_event_from_raw;
#[cfg(windows)]
use super::ClientLoopEvent;
use crate::input::WindowsKeyRecord;

#[cfg(windows)]
pub(super) fn raw_console_reader_loop(
    handle: windows_sys::Win32::Foundation::HANDLE,
    event_tx: mpsc::Sender<ClientLoopEvent>,
    should_quit: &Arc<AtomicBool>,
) {
    let mut mapper = WindowsInputMapper::default();
    let mut pump = WindowsInputPump::default();
    let mut handoff = WindowsInputHandoff::default();

    while !should_quit.load(Ordering::Acquire) {
        match windows_console_input_items(handle, &mut mapper) {
            WindowsInputItems::Items(items) => {
                process_platform_input_items(items, &mut pump, &mut handoff);
            }
            WindowsInputItems::Idle => {
                process_platform_input_items(mapper.idle(), &mut pump, &mut handoff);
                handoff.push(pump.idle());
            }
            WindowsInputItems::Closed => return,
        }
        if !handoff.try_flush(&event_tx) {
            return;
        }
    }
}

#[cfg(windows)]
fn process_platform_input_items(
    items: Vec<PlatformInputItem>,
    pump: &mut WindowsInputPump,
    handoff: &mut WindowsInputHandoff,
) {
    for item in items {
        handoff.push(pump.process(item));
    }
}

#[cfg(windows)]
pub(super) fn console_input_handle() -> std::io::Result<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE};

    let handle: HANDLE = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(handle)
    }
}

#[cfg(windows)]
pub(super) fn virtual_terminal_input_enabled(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> bool {
    use windows_sys::Win32::System::Console::{GetConsoleMode, ENABLE_VIRTUAL_TERMINAL_INPUT};

    let mut mode = 0;
    (unsafe { GetConsoleMode(handle, &mut mode) } != 0) && mode & ENABLE_VIRTUAL_TERMINAL_INPUT != 0
}

#[cfg(windows)]
enum WindowsInputItems {
    Items(Vec<PlatformInputItem>),
    Idle,
    Closed,
}

#[cfg(windows)]
#[derive(Default)]
struct WindowsInputHandoff {
    pending: VecDeque<Vec<crate::protocol::ClientInputEvent>>,
    backpressured: bool,
}

#[cfg(windows)]
impl WindowsInputHandoff {
    fn push(&mut self, events: Vec<crate::protocol::ClientInputEvent>) {
        if events.is_empty() {
            return;
        }
        if windows_input_trace_enabled() {
            tracing::info!(?events, "windows input trace: client input events");
        }
        if self.backpressured {
            self.push_backpressured(events);
        } else {
            self.pending.push_back(events);
        }
    }

    fn try_flush(&mut self, event_tx: &mpsc::Sender<ClientLoopEvent>) -> bool {
        // Keep draining the console while the client loop is busy. Blocking here
        // lets the OpenSSH/ConPTY input path lose pieces of raw VT reports.
        loop {
            if self.pending.is_empty() {
                self.backpressured = false;
                return true;
            }
            match event_tx.try_reserve() {
                Ok(permit) => {
                    let Some(events) = self.pending.pop_front() else {
                        continue;
                    };
                    permit.send(ClientLoopEvent::StdinEvents(events));
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    if !self.backpressured {
                        self.backpressured = true;
                        let pending = std::mem::take(&mut self.pending);
                        for events in pending {
                            self.push_backpressured(events);
                        }
                    }
                    return true;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return false,
            }
        }
    }

    fn push_backpressured(&mut self, events: Vec<crate::protocol::ClientInputEvent>) {
        if let Some(previous) = self.pending.back_mut() {
            if let ([previous_event], [next_event]) = (previous.as_slice(), events.as_slice()) {
                if windows_mouse_motion_can_replace(previous_event, next_event) {
                    *previous = events;
                    return;
                }
            }
        }
        self.pending.push_back(events);
    }
}

#[cfg(windows)]
fn windows_mouse_motion_can_replace(
    previous: &crate::protocol::ClientInputEvent,
    next: &crate::protocol::ClientInputEvent,
) -> bool {
    use crate::protocol::{ClientInputEvent, ClientMouseKind};

    let (
        ClientInputEvent::Mouse {
            kind: previous_kind,
            modifiers: previous_modifiers,
            ..
        },
        ClientInputEvent::Mouse {
            kind: next_kind,
            modifiers: next_modifiers,
            ..
        },
    ) = (previous, next)
    else {
        return false;
    };
    if previous_modifiers != next_modifiers {
        return false;
    }
    matches!(
        (previous_kind, next_kind),
        (ClientMouseKind::Moved, ClientMouseKind::Moved)
    ) || matches!(
        (previous_kind, next_kind),
        (ClientMouseKind::Drag(previous), ClientMouseKind::Drag(next)) if previous == next
    )
}

#[cfg(windows)]
fn windows_console_input_items(
    handle: windows_sys::Win32::Foundation::HANDLE,
    mapper: &mut WindowsInputMapper,
) -> WindowsInputItems {
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;

    match unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(handle, 10) } {
        WAIT_OBJECT_0 => {}
        WAIT_TIMEOUT => return WindowsInputItems::Idle,
        _ => return WindowsInputItems::Closed,
    }

    let mut records = [windows_sys::Win32::System::Console::INPUT_RECORD::default(); 64];
    let mut read = 0;
    let ok = unsafe {
        windows_sys::Win32::System::Console::ReadConsoleInputW(
            handle,
            records.as_mut_ptr(),
            records.len() as u32,
            &mut read,
        )
    };
    if ok == 0 {
        return WindowsInputItems::Closed;
    }

    let mut items = Vec::new();
    for record in records.iter().take(read as usize) {
        if let Some(record) = windows_console_input_record_from_os(*record) {
            items.extend(mapper.translate(record));
        }
    }
    WindowsInputItems::Items(items)
}

#[cfg(windows)]
fn windows_console_input_record_from_os(
    record: windows_sys::Win32::System::Console::INPUT_RECORD,
) -> Option<WindowsInputRecord> {
    use windows_sys::Win32::System::Console::{FOCUS_EVENT, KEY_EVENT, MOUSE_EVENT};

    match record.EventType as u32 {
        KEY_EVENT => {
            let key = unsafe { record.Event.KeyEvent };
            let unicode = unsafe { key.uChar.UnicodeChar };
            Some(WindowsInputRecord::Key(WindowsKeyRecord {
                key_down: key.bKeyDown != 0,
                repeat_count: key.wRepeatCount,
                virtual_key_code: key.wVirtualKeyCode,
                virtual_scan_code: key.wVirtualScanCode,
                unicode,
                control_key_state: key.dwControlKeyState,
            }))
        }
        MOUSE_EVENT => {
            let mouse = unsafe { record.Event.MouseEvent };
            Some(WindowsInputRecord::Mouse(WindowsMouseRecord {
                x: mouse.dwMousePosition.X.max(0) as u16,
                y: mouse.dwMousePosition.Y.max(0) as u16,
                button_state: mouse.dwButtonState,
                control_key_state: mouse.dwControlKeyState,
                event_flags: mouse.dwEventFlags,
            }))
        }
        FOCUS_EVENT => {
            let focus = unsafe { record.Event.FocusEvent };
            Some(WindowsInputRecord::Focus(focus.bSetFocus != 0))
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
enum WindowsInputRecord {
    Key(WindowsKeyRecord),
    Mouse(WindowsMouseRecord),
    Focus(bool),
}

#[derive(Clone, Copy, Debug)]
struct WindowsMouseRecord {
    x: u16,
    y: u16,
    button_state: u32,
    control_key_state: u32,
    event_flags: u32,
}

#[derive(Default)]
struct WindowsInputMapper {
    pending_high_surrogate: Option<u16>,
    pending_paste_high_surrogate: Option<u16>,
    mouse_buttons: WindowsMouseButtons,
    win32_input: WindowsWin32InputModeFramer,
}

struct WindowsInputPump {
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum PlatformInputItem {
    Bytes(Vec<u8>),
    Semantic(crate::protocol::ClientInputEvent),
    PasteAwareBytes {
        paste_bytes: Vec<u8>,
        raw_bytes: Vec<u8>,
        win32_paste_bytes: Vec<u8>,
    },
    PasteAwareKey {
        bytes: Vec<u8>,
        win32_paste_bytes: Vec<u8>,
        events: Vec<crate::protocol::ClientInputEvent>,
    },
}

#[derive(Default)]
struct WindowsWin32InputModeFramer {
    buffer: Vec<u8>,
}

enum WindowsWin32InputModeItem {
    Bytes(Vec<u8>),
    Key {
        bytes: Vec<u8>,
        record: WindowsKeyRecord,
    },
}

impl WindowsInputPump {
    fn process(&mut self, item: PlatformInputItem) -> Vec<crate::protocol::ClientInputEvent> {
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

    fn idle(&mut self) -> Vec<crate::protocol::ClientInputEvent> {
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

#[cfg(test)]
#[derive(Default)]
struct WindowsInputTranslator {
    mapper: WindowsInputMapper,
    pump: WindowsInputPump,
}

#[cfg(test)]
impl WindowsInputTranslator {
    fn translate(&mut self, record: WindowsInputRecord) -> Vec<crate::protocol::ClientInputEvent> {
        let mut events = Vec::new();
        for item in self.mapper.translate(record) {
            events.extend(self.pump.process(item));
        }
        events
    }

    fn idle(&mut self) -> Vec<crate::protocol::ClientInputEvent> {
        let mut events = Vec::new();
        for item in self.mapper.idle() {
            events.extend(self.pump.process(item));
        }
        events.extend(self.pump.idle());
        events
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct WindowsMouseButtons {
    left: bool,
    right: bool,
    middle: bool,
}

impl WindowsInputMapper {
    fn idle(&mut self) -> Vec<PlatformInputItem> {
        self.win32_input
            .flush_timeout()
            .into_iter()
            .map(PlatformInputItem::Bytes)
            .collect()
    }

    fn translate(&mut self, record: WindowsInputRecord) -> Vec<PlatformInputItem> {
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

    fn translate_semantic_key_events(
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

impl WindowsWin32InputModeFramer {
    fn push(&mut self, bytes: &[u8]) -> Vec<WindowsWin32InputModeItem> {
        self.buffer.extend_from_slice(bytes);

        let mut items = Vec::new();
        while let Some(item) = self.next_item() {
            items.push(item);
        }
        items
    }

    fn next_item(&mut self) -> Option<WindowsWin32InputModeItem> {
        if self.buffer.is_empty() {
            return None;
        }

        if self.buffer.as_slice() == b"\x1b" || self.buffer.as_slice() == b"\x1b[" {
            return None;
        }

        if self.buffer.starts_with(b"\x1b[") {
            let mut cursor = 2;
            while cursor < self.buffer.len()
                && (self.buffer[cursor].is_ascii_digit() || self.buffer[cursor] == b';')
            {
                cursor += 1;
            }

            if cursor == self.buffer.len() {
                return None;
            }

            if self.buffer[cursor] == b'_' {
                let bytes = self.buffer[..=cursor].to_vec();
                let body = String::from_utf8_lossy(&self.buffer[2..cursor]);
                let item = parse_win32_input_mode_key_record(&body).map(|record| {
                    WindowsWin32InputModeItem::Key {
                        bytes: bytes.clone(),
                        record,
                    }
                });
                self.buffer.drain(..=cursor);
                return Some(item.unwrap_or(WindowsWin32InputModeItem::Bytes(bytes)));
            }
        }

        Some(WindowsWin32InputModeItem::Bytes(vec![self
            .buffer
            .remove(0)]))
    }

    fn flush_timeout(&mut self) -> Vec<Vec<u8>> {
        if self.buffer.is_empty() {
            Vec::new()
        } else {
            vec![std::mem::take(&mut self.buffer)]
        }
    }
}

fn parse_win32_input_mode_key_record(body: &str) -> Option<WindowsKeyRecord> {
    let mut fields = body.split(';');
    let virtual_key_code = fields.next()?.parse::<u16>().ok()?;
    let virtual_scan_code = fields.next()?.parse::<u16>().ok()?;
    let unicode = fields.next()?.parse::<u16>().ok()?;
    let key_down = fields.next()?.parse::<u16>().ok()? != 0;
    let control_key_state = fields.next()?.parse::<u32>().ok()?;
    let repeat_count = fields.next()?.parse::<u16>().ok()?;
    if fields.next().is_some() {
        return None;
    }

    Some(WindowsKeyRecord {
        key_down,
        repeat_count,
        virtual_key_code,
        virtual_scan_code,
        unicode,
        control_key_state,
    })
}

fn windows_key_modifiers(control_key_state: u32) -> crossterm::event::KeyModifiers {
    const RIGHT_ALT_PRESSED: u32 = 0x0001;
    const LEFT_ALT_PRESSED: u32 = 0x0002;
    const RIGHT_CTRL_PRESSED: u32 = 0x0004;
    const LEFT_CTRL_PRESSED: u32 = 0x0008;
    const SHIFT_PRESSED: u32 = 0x0010;

    let mut modifiers = crossterm::event::KeyModifiers::empty();
    let alt_gr = control_key_state & RIGHT_ALT_PRESSED != 0
        && control_key_state & LEFT_CTRL_PRESSED != 0
        && control_key_state & RIGHT_CTRL_PRESSED == 0;
    if control_key_state & SHIFT_PRESSED != 0 {
        modifiers |= crossterm::event::KeyModifiers::SHIFT;
    }
    if !alt_gr && control_key_state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
        modifiers |= crossterm::event::KeyModifiers::CONTROL;
    }
    // Treat right Alt as AltGr rather than a terminal Alt prefix.
    if control_key_state & LEFT_ALT_PRESSED != 0
        || (control_key_state & RIGHT_ALT_PRESSED != 0 && !alt_gr)
    {
        modifiers |= crossterm::event::KeyModifiers::ALT;
    }
    modifiers
}

fn windows_virtual_key_to_key_code(
    vk: u16,
    modifiers: crossterm::event::KeyModifiers,
) -> Option<crate::protocol::ClientKeyCode> {
    use crate::protocol::ClientKeyCode;
    Some(match vk {
        0x08 => ClientKeyCode::Backspace,
        0x09 if modifiers.contains(crossterm::event::KeyModifiers::SHIFT) => ClientKeyCode::BackTab,
        0x09 => ClientKeyCode::Tab,
        0x0d => ClientKeyCode::Enter,
        0x1b => ClientKeyCode::Esc,
        0x21 => ClientKeyCode::PageUp,
        0x22 => ClientKeyCode::PageDown,
        0x23 => ClientKeyCode::End,
        0x24 => ClientKeyCode::Home,
        0x25 => ClientKeyCode::Left,
        0x26 => ClientKeyCode::Up,
        0x27 => ClientKeyCode::Right,
        0x28 => ClientKeyCode::Down,
        0x2d => ClientKeyCode::Insert,
        0x2e => ClientKeyCode::Delete,
        0x70..=0x87 => ClientKeyCode::F((vk - 0x6f) as u8),
        _ => return None,
    })
}

fn windows_virtual_key_to_char_code(
    vk: u16,
    unicode: u16,
    modifiers: crossterm::event::KeyModifiers,
) -> Option<crate::protocol::ClientKeyCode> {
    use crate::protocol::ClientKeyCode;

    if let Some(ch) = char::from_u32(unicode as u32).filter(|ch| !ch.is_control()) {
        return Some(ClientKeyCode::Char(ch));
    }

    let ch = match vk {
        0x30..=0x39 => char::from_u32(vk as u32)?,
        0x41..=0x5a
            if modifiers.contains(crossterm::event::KeyModifiers::SHIFT)
                && !modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            char::from_u32(vk as u32)?
        }
        0x41..=0x5a => char::from_u32(vk as u32 + 32)?,
        _ => return None,
    };
    Some(ClientKeyCode::Char(ch))
}

fn ctrl_key_code(vk: u16, u: u16, oem: Option<char>) -> Option<crate::protocol::ClientKeyCode> {
    use crate::protocol::ClientKeyCode;
    Some(match (vk, u) {
        (0xbf, 0x00) => ClientKeyCode::Char(oem?),
        (_, 0x00) => ClientKeyCode::Char(' '),
        (_, 0x1b) => ClientKeyCode::Char('['),
        (_, 0x1c) => ClientKeyCode::Char('\\'),
        (_, 0x1d) => ClientKeyCode::Char(']'),
        (_, 0x1e) => ClientKeyCode::Char('^'),
        (_, 0x1f) => ClientKeyCode::Char('-'),
        _ => return None,
    })
}

fn resolve_ctrl_oem_char(key: WindowsKeyRecord) -> Option<char> {
    if key.virtual_key_code == 0xbf
        && key.unicode == 0
        && windows_key_modifiers(key.control_key_state)
            .contains(crossterm::event::KeyModifiers::CONTROL)
    {
        #[cfg(windows)]
        return crate::platform::resolve_base_printable_key(
            key.virtual_key_code,
            key.virtual_scan_code,
        );
    }
    None
}

#[cfg(any(windows, test))]
fn windows_input_trace_enabled() -> bool {
    std::env::var_os("HERDR_WINDOWS_INPUT_TRACE").is_some()
}

#[cfg(test)]
#[path = "tests/support_test.rs"]
mod tests;
