//! A complete consumer: terminal, input, keys and placement belong to this app.
use markdown_ratatui::{LayoutError, MarkdownView, ParseError, ViewState};
use ratatui::{
    crossterm::event::{self, Event, KeyCode},
    layout::{Constraint, Layout},
    widgets::{Block, Paragraph},
};
use std::{fmt, io};

#[derive(Debug)]
enum Error {
    Io(io::Error),
    Parse(ParseError),
    Layout(LayoutError),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Parse(e) => e.fmt(f),
            Self::Layout(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse(e) => Some(e),
            Self::Layout(e) => Some(e),
        }
    }
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<LayoutError> for Error {
    fn from(e: LayoutError) -> Self {
        Self::Layout(e)
    }
}
impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}
struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        ratatui::restore();
    }
}
fn main() -> Result<(), Error> {
    let mut markdown = MarkdownView::new(
        "# Embedded Markdown\n\nThe caller owns **this screen**, its state and its keys.\n\n日本語 é 👩‍💻\n\n[An inert link](https://example.com)\n\n- No files or repositories required\n- No Tokio runtime\n- No automatic link opening\n\n> Scroll with the arrows; select a link with Tab.\n\n```rust\nlet view = MarkdownView::new(source)?;\n```\n\nEnd of example.",
    )?;
    let mut state = ViewState::default();
    if std::env::args().any(|a| a == "--buffer") {
        use ratatui::{buffer::Buffer, layout::Rect, widgets::StatefulWidget};
        let area = Rect::new(2, 1, 44, 20);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 48, 22));
        markdown
            .prepare(area.width)?
            .widget()
            .render(area, &mut buffer, &mut state);
        for row in 0..buffer.area.height {
            println!(
                "{}",
                (0..buffer.area.width)
                    .map(|x| buffer[(x, row)].symbol())
                    .collect::<String>()
            );
        }
        return Ok(());
    }
    let _restore = Restore;
    let mut terminal = ratatui::try_init()?;
    loop {
        let width = terminal.size()?.width.saturating_sub(20);
        let layout = markdown.prepare(width)?;
        terminal.draw(|frame| {
            let [sidebar, body] = Layout::horizontal([Constraint::Length(20), Constraint::Min(0)])
                .areas(frame.area());
            frame.render_widget(
                Paragraph::new("Your application\n\n↑/↓ Scroll\nTab Select link\nq Quit")
                    .block(Block::bordered()),
                sidebar,
            );
            frame.render_stateful_widget(layout.widget(), body, &mut state);
        })?;
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Down => state.scroll_forward(1),
                KeyCode::Up => state.scroll_backward(1),
                KeyCode::Tab => {
                    state.select_link(markdown.document().links().first().map(|link| link.id()))
                }
                _ => {}
            }
        }
    }
    ratatui::try_restore()?;
    Ok(())
}
