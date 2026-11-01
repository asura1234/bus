//! Styled terminal cells shared by pane observation and the scratch UI driver.
use unicode_width::UnicodeWidthStr;

pub(crate) use crate::protocol::api::schema::driver::{Cell, Cursor, Grid};

impl Grid {
    #[cfg(any(test, feature = "dev-tools"))]
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

    /// Read original VT cells; presentation color transforms and dirty acknowledgement
    /// belong to the renderer, not to observation.
    pub(crate) fn from_terminal(
        terminal: &crate::terminal::vt::Terminal,
    ) -> Result<Self, crate::terminal::vt::Error> {
        use crate::terminal::vt::{CellColor, CellWide, KITTY_UNICODE_PLACEHOLDER};
        fn color(color: Option<CellColor>) -> String {
            match color {
                None => "default".into(),
                Some(CellColor::Palette(index)) => format!("idx:{index}"),
                Some(CellColor::Rgb(rgb)) => format!("#{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b),
            }
        }
        let cols = terminal.cols()?;
        let rows = terminal.rows()?;
        let mut cells = Vec::with_capacity(usize::from(rows));
        for y in 0..rows {
            let mut row = Vec::with_capacity(usize::from(cols));
            for x in 0..cols {
                let (wide, graphemes, style) = terminal.viewport_styled_cell(x, u32::from(y))?;
                let symbol = if wide == CellWide::SpacerTail {
                    String::new()
                } else if graphemes.is_empty()
                    || graphemes.first() == Some(&KITTY_UNICODE_PLACEHOLDER)
                {
                    " ".into()
                } else {
                    graphemes.into_iter().filter_map(char::from_u32).collect()
                };
                let attrs = [
                    (style.bold, "bold"),
                    (style.faint, "dim"),
                    (style.italic, "italic"),
                    (style.underlined, "underline"),
                    (style.inverse, "inverse"),
                    (style.strikethrough, "strike"),
                    (style.blink, "blink"),
                    (style.invisible, "hidden"),
                    (style.overline, "overline"),
                ]
                .into_iter()
                .filter(|(set, _)| *set)
                .map(|(_, name)| name.to_owned())
                .collect();
                row.push(Cell {
                    symbol,
                    fg: color(style.fg_color),
                    bg: color(style.bg_color),
                    attrs,
                    link: terminal.viewport_hyperlink_uri(x, u32::from(y))?,
                });
            }
            cells.push(row);
        }
        let scrollbar = terminal.scrollbar()?;
        let from_bottom = scrollbar
            .total
            .saturating_sub(scrollbar.offset + scrollbar.len);
        let y = usize::from(terminal.cursor_y()?).saturating_add(from_bottom);
        let cursor = if y < usize::from(rows) {
            Some(Cursor {
                row: y as u16,
                col: terminal.cursor_x()?,
                visible: terminal.mode_get(25)?,
            })
        } else {
            None
        };
        Ok(Self {
            cols,
            rows,
            cells,
            cursor,
        })
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
