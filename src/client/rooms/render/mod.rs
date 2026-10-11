//! Room render model and the stable room-facing render API.
mod dialogs;
#[path = "layout.rs"]
mod geometry;
mod paint;
mod room;
mod sidebar;
mod text;
mod view;

pub(in crate::client) use geometry::layout;
pub(super) use sidebar::sidebar_room_row;
pub(super) use text::{
    cell_offset, cell_width, display, provider, status, wrap, wrap_ranges, wrapped_position,
};

use super::deletion::DeleteTarget;
use crate::messaging::model::{Provider, RoomId};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// Background of mouse-selected text.
const SELECTION: Color = Color::Rgb(44, 88, 56);
const ACCENT: Color = {
    let [r, g, b] = crate::messaging::prefs::colors::YOU_COLOR;
    Color::Rgb(r, g, b)
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Delete(DeleteTarget),
    CancelDelete,
    ConfirmDelete,
    DismissAlert,
    Room(crate::messaging::model::RoomId),
    Agent(crate::messaging::model::AgentId),
    NewRoom,
    NewAgent,
    ScrollFiles(bool),
    Notes,
    Composer,
    Recipients,
    Recipient(Option<crate::messaging::model::AgentId>),
    Files,
    RemoveFile(std::path::PathBuf),
    /// A message attachment; clicking opens it in its default app.
    OpenFile(std::path::PathBuf),
    Details(crate::messaging::model::AgentId),
    Quote(crate::messaging::model::RequestId),
    Field(usize),
    Provider(Provider),
    Orchestrates,
    Suggestion(usize),
    Settings,
    ToggleColorBlindMode,
    ToggleSound(SoundTarget),
    /// Picks the next (true) or previous system sound for a Settings sound row.
    CycleSound(SoundTarget, bool),
    AdjustCompactionLimit(bool),
    Cancel,
    Add,
}
#[derive(Clone, Debug)]
pub(super) struct Hit {
    pub rect: Rect,
    pub action: Action,
}
#[derive(Clone)]
struct Row {
    x: u16,
    y: u16,
    width: u16,
    text: String,
    selected: bool,
    muted: bool,
    color: Option<Color>,
    character_colors: Option<Vec<Color>>,
    style: Option<Style>,
}
#[derive(Default)]
pub(super) struct View {
    pub hits: Vec<Hit>,
    pub sidebar: Rect,
    pub sidebar_body: Rect,
    pub sidebar_max_scroll: usize,
    sidebar_divider: Rect,
    settings_divider: Rect,
    pub history: Rect,
    pub history_max_scroll: usize,
    pub recipient_bar: Rect,
    pub recipient_max_scroll: u16,
    recipient_chips: Vec<(Rect, Color)>,
    pub files: Rect,
    pub composer: Rect,
    pub composer_max_height: u16,
    pub composer_scroll: usize,
    pub composer_rows: usize,
    composer_box: Rect,
    composer_divider: Rect,
    pub notes_box: Rect,
    history_divider: Rect,
    dialog: Rect,
    dialog_rows_start: usize,
    pub help: Rect,
    pub help_scroll: usize,
    pub help_max_scroll: usize,
    /// The scrollable sound-notification rows of the Settings form.
    pub settings_list: Rect,
    pub settings_max_scroll: usize,
    rows: Vec<Row>,
    pub cursor: Option<crate::protocol::wire::CursorState>,
    pub notes: Rect,
    pub notes_scroll: usize,
    /// Wrapped rows of the room notes, of which `notes` shows a window.
    pub notes_rows: usize,
    /// Text columns of the visible history rows.
    pub history_text: Rect,
    /// Clickable URLs and paths in the visible history rows.
    pub history_links: Vec<(Rect, super::links::Target)>,
    /// The chat search panel above the history; empty while it is closed.
    pub search_box: Rect,
    /// The bordered query field inside `search_box`, after the "Find" label.
    pub search_field: Rect,
    /// The chat search's matches over all history rows, as (row, byte range),
    /// laid out at the last render's width.
    pub search_matches: Vec<(usize, std::ops::Range<usize>)>,
    /// Selected cells, painted after the rows they cover.
    selection: Vec<Rect>,
    /// Image thumbnails wholly inside the history viewport.
    pub thumbnails: Vec<super::thumbnails::Placement>,
}

/// What a Settings sound row sets: a room's sound (MASTER's is global), or
/// every work room at once, which is also what new work rooms start with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SoundTarget {
    Room(RoomId),
    AllRooms,
}

/// One row of the Settings form's sound-notification list.
pub(super) enum SoundSettingsLine {
    Heading(&'static str),
    Empty(&'static str),
    /// `field` is the row's Settings focus index; color blind mode is field 0.
    Sound {
        target: SoundTarget,
        field: usize,
    },
    Compactions {
        field: usize,
    },
}
