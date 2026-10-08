//! Console draining and lossless queue handoff under event-loop backpressure.
use super::super::ClientLoopEvent;
use super::records::{windows_console_input_record_from_os, PlatformInputItem};
use super::{
    keymap::windows_input_trace_enabled, mapper::WindowsInputMapper, pump::WindowsInputPump,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

#[cfg(windows)]
pub(in crate::client::host_terminal::input) fn raw_console_reader_loop(
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
pub(in crate::client::host_terminal::input) fn console_input_handle(
) -> std::io::Result<windows_sys::Win32::Foundation::HANDLE> {
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
pub(in crate::client::host_terminal::input) fn virtual_terminal_input_enabled(
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
pub(super) struct WindowsInputHandoff {
    pub(super) pending: VecDeque<Vec<crate::protocol::ClientInputEvent>>,
    backpressured: bool,
}

#[cfg(windows)]
impl WindowsInputHandoff {
    pub(super) fn push(&mut self, events: Vec<crate::protocol::ClientInputEvent>) {
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

    pub(super) fn try_flush(&mut self, event_tx: &mpsc::Sender<ClientLoopEvent>) -> bool {
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
