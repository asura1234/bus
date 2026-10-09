//! Windows console input: OS reader, record mapping, framing, and delivery.
mod keymap;
mod mapper;
mod pump;
#[cfg(windows)]
mod reader;
mod records;
mod win32_input_mode;

#[cfg(windows)]
pub(super) use reader::{
    console_input_handle, raw_console_reader_loop, virtual_terminal_input_enabled,
};

#[cfg(all(test, windows))]
use super::ClientLoopEvent;
#[cfg(test)]
use crate::protocol::keys::WindowsKeyRecord;
#[cfg(test)]
use mapper::WindowsInputMapper;
#[cfg(test)]
use pump::WindowsInputPump;
#[cfg(all(test, windows))]
use reader::WindowsInputHandoff;
#[cfg(test)]
use records::{WindowsInputRecord, WindowsMouseRecord};
#[cfg(all(test, windows))]
use std::collections::VecDeque;
#[cfg(all(test, windows))]
use tokio::sync::mpsc;

#[cfg(test)]
#[derive(Default)]
struct WindowsInputTranslator {
    mapper: WindowsInputMapper,
    pump: WindowsInputPump,
}

#[cfg(test)]
impl WindowsInputTranslator {
    fn translate(
        &mut self,
        record: WindowsInputRecord,
    ) -> Vec<crate::protocol::wire::ClientInputEvent> {
        let mut events = Vec::new();
        for item in self.mapper.translate(record) {
            events.extend(self.pump.process(item));
        }
        events
    }

    fn idle(&mut self) -> Vec<crate::protocol::wire::ClientInputEvent> {
        let mut events = Vec::new();
        for item in self.mapper.idle() {
            events.extend(self.pump.process(item));
        }
        events.extend(self.pump.idle());
        events
    }
}

#[cfg(test)]
#[path = "tests/support_test.rs"]
mod tests;
