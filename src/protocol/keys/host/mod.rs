mod event;
mod framer;
mod mouse;
mod replies;
mod sequence;

const ESC: u8 = 0x1b;
#[cfg(unix)]
pub(crate) const RAW_INPUT_IDLE_FLUSH_TIMEOUT_MS: i32 = 10;
#[cfg(unix)]
pub(crate) const MOUSE_ACTIVE_ESCAPE_SEQUENCE_FLUSH_TIMEOUT_MS: i32 = 150;
pub(crate) const GHOSTTY_COLOR_SCHEME_DARK_REPORT: &[u8] = b"\x1b[?997;1n";
pub(crate) const GHOSTTY_COLOR_SCHEME_LIGHT_REPORT: &[u8] = b"\x1b[?997;2n";
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

#[cfg(test)]
use event::extract_one_event;
pub use event::RawInputEvent;
#[cfg(any(unix, test))]
pub use framer::parse_raw_input_bytes_sync;
#[cfg(any(not(windows), test))]
pub(crate) use framer::RawInputByteFramer;
#[cfg(any(windows, test))]
pub(crate) use framer::RawInputFramer;
#[cfg(test)]
use replies::parse_host_cell_size_report;

#[cfg(test)]
use crate::utils::theme::color::{DefaultColorKind, HostAppearance, RgbColor};
#[cfg(test)]
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

#[cfg(test)]
#[path = "../tests/host_framer_test.rs"]
mod tests;
