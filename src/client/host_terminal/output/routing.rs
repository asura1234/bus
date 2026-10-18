//! Route event-time terminal effects through the same FIFO as frames.
use super::queue::Handle;
use std::{
    cell::RefCell,
    io::{self, Write},
};

thread_local! { static ACTIVE: RefCell<Option<Handle>> = const { RefCell::new(None) }; }

pub(in crate::client) struct Route(Option<Handle>);
impl Route {
    pub(super) fn new(handle: Handle) -> Self {
        Self(ACTIVE.with(|active| active.replace(Some(handle))))
    }
}
impl Drop for Route {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.replace(self.0.take()));
    }
}

pub(in crate::client) fn busy() -> bool {
    ACTIVE.with(|active| active.borrow().as_ref().is_some_and(Handle::busy))
}

pub(in crate::client) fn stdout() -> HostWriter {
    HostWriter {
        handle: ACTIVE.with(|active| active.borrow().clone()),
        bytes: Vec::new(),
    }
}

pub(in crate::client) struct HostWriter {
    handle: Option<Handle>,
    bytes: Vec<u8>,
}
impl Write for HostWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > 32 * 1024 * 1024 {
            let error = io::Error::other("host terminal effect exceeds memory limit");
            if let Some(handle) = &self.handle {
                return handle.fail(error).map(|()| 0);
            }
            return Err(error);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.bytes.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.bytes);
        if let Some(handle) = &self.handle {
            handle.push(super::queue::Command::Bytes(bytes))
        } else {
            let mut stdout = io::stdout();
            stdout.write_all(&bytes)?;
            stdout.flush()
        }
    }
}
impl Drop for HostWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// Terminal restoration (including the panic hook) takes exclusive ownership
/// only after all admitted output finishes. Cleanup then uses real stdout.
pub(in crate::client) fn suspend() -> Route {
    let handle = ACTIVE.with(|active| active.borrow().clone());
    if let Some(handle) = &handle {
        handle.wait_idle();
    }
    Route(ACTIVE.with(|active| active.replace(None)))
}
