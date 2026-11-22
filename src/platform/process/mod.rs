#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

mod pane_shutdown;
use pane_shutdown::PaneProcessInfo;
pub(crate) use pane_shutdown::PaneProcessTree;

#[cfg(test)]
pub(crate) use pane_shutdown::signal_processes;
