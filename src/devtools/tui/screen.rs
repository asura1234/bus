//! The driver's view of the Bus client: libghostty-vt fed with the client's
//! PTY output, read back as text rows and styled cells.
use crate::terminal::emulator::GhosttyPaneTerminal;
use crate::utils::ids::PaneId;
use bytes::Bytes;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

/// `CSI ? 2026 l`: the client ends every frame with it.
const FRAME_END: &[u8] = b"\x1b[?2026l";
/// `CSI ? 2026 h`: the client starts every frame with it.
const FRAME_BEGIN: &[u8] = b"\x1b[?2026h";

pub(super) struct Screen {
    pane: GhosttyPaneTerminal,
    pane_id: PaneId,
    responses: tokio::sync::mpsc::Sender<Bytes>,
    cols: u16,
    rows: u16,
    frame_tail: Vec<u8>,
    /// The start of a frame-start marker the last read split; it is fed with
    /// the next read, so the marker is always seen whole.
    begin_tail: Vec<u8>,
    /// The last complete frame, kept while the next one is being drawn.
    complete: Option<Grid>,
}

pub(super) struct Feed {
    pub responses: Vec<Bytes>,
    pub clipboard_writes: Vec<Vec<u8>>,
    pub bells: u16,
    pub frames: u64,
}

pub(super) use crate::protocol::api::schema::driver::{Cell, Cursor, Grid};

impl Screen {
    pub fn new(cols: u16, rows: u16) -> std::io::Result<Self> {
        let terminal = crate::terminal::vt::Terminal::new(cols, rows, 0)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        // The emulator answers queries through its own callback and hands the
        // replies back from `process_pty_bytes`; this channel is never read.
        let (responses, _unused) = tokio::sync::mpsc::channel(1);
        let ghostty = GhosttyPaneTerminal::new(terminal, responses.clone())?;
        let mut screen = Self {
            pane: ghostty,
            pane_id: PaneId::alloc(),
            responses,
            cols,
            rows,
            frame_tail: Vec::new(),
            begin_tail: Vec::new(),
            complete: None,
        };
        screen.resize(cols, rows);
        Ok(screen)
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Feed {
        let frames = count_frame_ends(&mut self.frame_tail, bytes);
        let mut feed = Feed {
            responses: Vec::new(),
            clipboard_writes: Vec::new(),
            bells: 0,
            frames,
        };
        let mut joined = std::mem::take(&mut self.begin_tail);
        joined.extend_from_slice(bytes);
        let held = split_begin_prefix(&joined);
        self.begin_tail = joined.split_off(joined.len() - held);
        // Feed up to each frame start; when one starts after a complete
        // screen, keep that screen for readers until the new frame ends.
        let mut rest = joined.as_slice();
        while !rest.is_empty() {
            let cut = rest
                .windows(FRAME_BEGIN.len())
                .skip(1)
                .position(|window| window == FRAME_BEGIN)
                .map_or(rest.len(), |at| at + 1);
            let (segment, tail) = rest.split_at(cut);
            if segment.starts_with(FRAME_BEGIN) && !self.pane.synchronized_output_active() {
                self.complete = Some(self.render());
            }
            let result =
                self.pane
                    .process_pty_bytes(self.pane_id, 0, segment, &self.responses, |_| None);
            feed.responses.extend(result.terminal_responses);
            feed.clipboard_writes.extend(result.clipboard_writes);
            feed.bells = feed.bells.saturating_add(result.terminal_bells);
            rest = tail;
        }
        if !self.pane.synchronized_output_active() {
            self.complete = None;
        }
        feed
    }

    /// Resizes the screen the way a host terminal does: no pane reflow replay.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.complete = None;
        if let Ok(mut core) = self.pane.core.lock() {
            let _ = core
                .terminal
                .resize(cols, rows, CELL_WIDTH_PX, CELL_HEIGHT_PX);
        }
    }

    pub fn bracketed_paste(&self) -> bool {
        self.pane.bracketed_paste_enabled()
    }

    pub fn keyboard_protocol(&self) -> crate::protocol::keys::KeyboardProtocol {
        self.pane
            .keyboard_protocol()
            .unwrap_or(crate::protocol::keys::KeyboardProtocol::Legacy)
    }

    /// The screen as of the last complete frame: a frame still being drawn
    /// (synchronized output on) is never shown half done.
    pub fn grid(&self) -> Grid {
        self.complete.clone().unwrap_or_else(|| self.render())
    }

    fn render(&self) -> Grid {
        let area = Rect::new(0, 0, self.cols, self.rows);
        let Ok(mut terminal) = ratatui::Terminal::new(TestBackend::new(self.cols, self.rows));
        let drawn = terminal.draw(|frame| self.pane.render(frame, area, false));
        if drawn.is_err() {
            return Grid::blank(self.cols, self.rows);
        }
        let links = self.pane.visible_hyperlinks(area);
        let mut grid = grid_from_buffer(terminal.backend().buffer());
        for ((x, y), _, uri) in links {
            if let Some(cell) = grid
                .cells
                .get_mut(usize::from(y))
                .and_then(|row| row.get_mut(usize::from(x)))
            {
                cell.link = Some(uri);
            }
        }
        grid.cursor = self.pane.cursor_state().map(|cursor| Cursor {
            row: cursor.y,
            col: cursor.x,
            visible: cursor.visible,
        });
        grid
    }
}

pub(super) const CELL_WIDTH_PX: u32 = 8;
pub(super) const CELL_HEIGHT_PX: u32 = 16;

/// Length of the trailing bytes that could start a split frame-start marker.
fn split_begin_prefix(bytes: &[u8]) -> usize {
    (1..FRAME_BEGIN.len().min(bytes.len() + 1))
        .rev()
        .find(|len| bytes.ends_with(&FRAME_BEGIN[..*len]))
        .unwrap_or(0)
}

/// Counts frame ends across chunk boundaries; `tail` keeps the bytes that
/// could start a split marker.
pub(super) fn count_frame_ends(tail: &mut Vec<u8>, bytes: &[u8]) -> u64 {
    let mut joined = std::mem::take(tail);
    joined.extend_from_slice(bytes);
    let count = joined
        .windows(FRAME_END.len())
        .filter(|window| *window == FRAME_END)
        .count() as u64;
    let keep = (FRAME_END.len() - 1).min(joined.len());
    *tail = joined[joined.len() - keep..].to_vec();
    count
}

/// Where a match sits on screen, in cells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Match {
    pub row: u16,
    pub col: u16,
    pub width: u16,
    pub text: String,
}

pub(super) enum Needle<'a> {
    Text(&'a str),
    Regex(regex::Regex),
}

impl Needle<'_> {
    fn find_in(&self, haystack: &str) -> Vec<(usize, usize)> {
        match self {
            Needle::Text("") => Vec::new(),
            Needle::Text(text) => haystack
                .match_indices(*text)
                .map(|(start, found)| (start, start + found.len()))
                .collect(),
            Needle::Regex(regex) => regex
                .find_iter(haystack)
                .filter(|m| !m.is_empty())
                .map(|m| (m.start(), m.end()))
                .collect(),
        }
    }
}

/// Every match of `needle` in rows `rows` (inclusive range of row indexes).
pub(super) fn find(grid: &Grid, needle: &Needle<'_>, rows: Option<(u16, u16)>) -> Vec<Match> {
    let (first, last) = rows.unwrap_or((0, grid.rows.saturating_sub(1)));
    let mut matches = Vec::new();
    for row in first..=last.min(grid.rows.saturating_sub(1)) {
        let (text, columns) = grid.row_with_columns(usize::from(row));
        for (start, end) in needle.find_in(&text) {
            let col = columns[start];
            let found = &text[start..end];
            matches.push(Match {
                row,
                col,
                width: found.width() as u16,
                text: found.to_owned(),
            });
        }
    }
    matches
}

#[cfg(test)]
#[path = "tests/screen_test.rs"]
mod tests;

fn grid_from_buffer(buffer: &ratatui::buffer::Buffer) -> Grid {
    let area = buffer.area;
    let cells = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| {
                    let cell = &buffer[(x, y)];
                    Cell {
                        symbol: cell.symbol().to_owned(),
                        fg: color_name(cell.fg),
                        bg: color_name(cell.bg),
                        attrs: attrs(cell.modifier),
                        link: None,
                    }
                })
                .collect()
        })
        .collect();
    Grid {
        cols: area.width,
        rows: area.height,
        cells,
        cursor: None,
    }
}

fn color_name(color: Color) -> String {
    match color {
        Color::Reset => "default".into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(index) => format!("idx:{index}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn attrs(modifier: Modifier) -> Vec<String> {
    [
        (Modifier::BOLD, "bold"),
        (Modifier::DIM, "dim"),
        (Modifier::ITALIC, "italic"),
        (Modifier::UNDERLINED, "underline"),
        (Modifier::REVERSED, "inverse"),
        (Modifier::CROSSED_OUT, "strike"),
        (Modifier::SLOW_BLINK, "blink"),
        (Modifier::HIDDEN, "hidden"),
    ]
    .into_iter()
    .filter(|(flag, _)| modifier.contains(*flag))
    .map(|(_, name)| name.to_owned())
    .collect()
}
