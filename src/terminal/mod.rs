mod history_read;
mod runtime;
mod runtime_registry;
pub mod state;
mod title;

pub use crate::utils::ids::TerminalId;
pub(crate) use history_read::{merge_scrolled_up, snapshot_text, ScreenSnapshot, UpwardMerge};
pub use runtime::TerminalRuntime;
pub(crate) use runtime_registry::TerminalRuntimeRegistry;
pub use state::{
    EffectivePresentation, EffectiveStateChange, TerminalState, TerminalStateMutation,
};
pub(crate) use title::stripped_terminal_title;
