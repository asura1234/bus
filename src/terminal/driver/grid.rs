//! Styled terminal cells shared by pane observation and the scratch UI driver.
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

pub(crate) use crate::protocol::api::schema::driver::{Cell, Cursor, Grid};

impl Grid {
    #[cfg(test)]
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
