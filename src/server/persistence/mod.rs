//! Session persistence — save/restore workspaces, layouts, and working directories.
//!
//! Stored at `~/.config/herdr/session.json`.
//! Optional pane screen history is stored separately at `session-history.json`.

mod autosave;
mod capture;
mod restore;
mod schema;
mod store;

pub use self::capture::{capture, capture_history};
pub use self::restore::restore;
pub use self::schema::{
    DirectionSnapshot, LayoutSnapshot, SessionHistorySnapshot, SessionSnapshot, TabSnapshot,
    WorkspaceSnapshot,
};
pub use self::store::{clear, clear_history, load, load_history, save};
