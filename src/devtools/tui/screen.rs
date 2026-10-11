//! The driver's view of the Bus client: libghostty-vt fed with the client's
//! PTY output, read back as text rows and styled cells.
use crate::terminal::emulator::GhosttyPaneTerminal;
use crate::utils::ids::PaneId;
use bytes::Bytes;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

/// `CSI ? 2026 l`: the client ends every frame with it.
const FRAME_END: &[u8] = b"\x1b[?2026l";

pub(super) struct Screen {
    pane: GhosttyPaneTerminal,
    pane_id: PaneId,
    responses: tokio::sync::mpsc::Sender<Bytes>,
    cols: u16,
    rows: u16,
    frame_tail: Vec<u8>,
}

pub(super) struct Feed {
    pub responses: Vec<Bytes>,
    pub clipboard_writes: Vec<Vec<u8>>,
    pub bells: u16,
    pub frames: u64,
}

/// One rendered cell. Wide characters occupy their first cell; the cell after
/// them has an empty symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Cell {
    pub symbol: String,
    pub fg: String,
    pub bg: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Cursor {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Vec<Cell>>,
    pub cursor: Option<Cursor>,
}

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
        };
        screen.resize(cols, rows);
        Ok(screen)
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Feed {
        let frames = count_frame_ends(&mut self.frame_tail, bytes);
        let result = self
            .pane
            .process_pty_bytes(self.pane_id, 0, bytes, &self.responses, |_| None);
        Feed {
            responses: result.terminal_responses,
            clipboard_writes: result.clipboard_writes,
            bells: result.terminal_bells,
            frames,
        }
    }

    /// Resizes the screen the way a host terminal does: no pane reflow replay.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
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

    pub fn grid(&self) -> Grid {
        let area = Rect::new(0, 0, self.cols, self.rows);
        let Ok(mut terminal) = ratatui::Terminal::new(TestBackend::new(self.cols, self.rows));
        let drawn = terminal.draw(|frame| self.pane.render(frame, area, false));
        if drawn.is_err() {
            return Grid::blank(self.cols, self.rows);
        }
        let links = self.pane.visible_hyperlinks(area);
        let mut grid = Grid::from_buffer(terminal.backend().buffer());
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

impl Grid {
    pub fn blank(cols: u16, rows: u16) -> Self {
        let cell = Cell {
            symbol: " ".into(),
            fg: "default".into(),
            bg: "default".into(),
            attrs: Vec::new(),
            link: None,
        };
        Self {
            cols,
            rows,
            cells: vec![vec![cell; usize::from(cols)]; usize::from(rows)],
            cursor: None,
        }
    }

    pub fn from_buffer(buffer: &Buffer) -> Self {
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
        Self {
            cols: area.width,
            rows: area.height,
            cells,
            cursor: None,
        }
    }

    /// Row text with trailing blanks trimmed. Wide tails contribute nothing.
    pub fn row_text(&self, row: usize) -> String {
        self.row_with_columns(row).0.trim_end().to_owned()
    }

    pub fn rows_text(&self) -> Vec<String> {
        (0..self.cells.len())
            .map(|row| self.row_text(row))
            .collect()
    }

    /// Row text plus the cell column where each byte of it starts.
    pub fn row_with_columns(&self, row: usize) -> (String, Vec<u16>) {
        let mut text = String::new();
        let mut columns = Vec::new();
        if let Some(cells) = self.cells.get(row) {
            let mut covered = 0;
            for (x, cell) in cells.iter().enumerate() {
                // The cells under a wide character's second half hold no text.
                if covered > 0 {
                    covered -= 1;
                    continue;
                }
                if cell.symbol.is_empty() {
                    continue;
                }
                covered = cell.symbol.width().saturating_sub(1);
                for _ in 0..cell.symbol.len() {
                    columns.push(x as u16);
                }
                text.push_str(&cell.symbol);
            }
        }
        (text, columns)
    }

    pub fn text(&self) -> String {
        self.rows_text().join("\n")
    }
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

fn color_name(color: Color) -> String {
    match color {
        Color::Reset => "default".into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(index) => format!("idx:{index}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn attrs(modifier: Modifier) -> Vec<&'static str> {
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
    .map(|(_, name)| name)
    .collect()
}

#[cfg(test)]
#[path = "tests/screen_test.rs"]
mod tests;
