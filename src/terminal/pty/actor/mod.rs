mod input_completion;
pub(crate) use input_completion::InputCompletion;

/// Bytes read from all PTYs; a burst of agent output shows up here.
pub(crate) static PTY_BYTES: crate::utils::perf::Counter =
    crate::utils::perf::Counter::new("terminal.pty_bytes");
/// Feeding one PTY read into its terminal, including the wait for the response order lock.
pub(crate) static PTY_READ: crate::utils::perf::Timer =
    crate::utils::perf::Timer::new("terminal.pty_read");

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub(crate) use unix::*;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) use windows::*;
