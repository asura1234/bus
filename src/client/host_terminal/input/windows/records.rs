//! Windows console records and the semantic/byte handoff to the pump.
use crate::input::WindowsKeyRecord;

#[cfg(windows)]
pub(super) fn windows_console_input_record_from_os(
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
pub(super) enum WindowsInputRecord {
    Key(WindowsKeyRecord),
    Mouse(WindowsMouseRecord),
    Focus(bool),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct WindowsMouseRecord {
    pub(super) x: u16,
    pub(super) y: u16,
    pub(super) button_state: u32,
    pub(super) control_key_state: u32,
    pub(super) event_flags: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlatformInputItem {
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
