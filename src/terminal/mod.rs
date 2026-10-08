pub(crate) mod emulator;
pub(crate) mod events;
mod history;
pub(crate) mod pty;
mod registry;
pub(crate) mod runtime;
pub mod state;
pub(crate) mod vt;

pub(crate) use crate::agents::title::stripped_terminal_title;
pub use crate::utils::ids::TerminalId;
pub(crate) use history::{merge_scrolled_up, snapshot_text, ScreenSnapshot, UpwardMerge};
pub(crate) use registry::TerminalRuntimeRegistry;
pub use runtime::TerminalRuntime;
pub use state::{
    EffectivePresentation, EffectiveStateChange, TerminalState, TerminalStateMutation,
};
