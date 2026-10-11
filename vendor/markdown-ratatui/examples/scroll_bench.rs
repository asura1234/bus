//! Reproducible, backend-free measurement; not a statistically rigorous benchmark.
use markdown_ratatui::{MarkdownView, ViewState};
use ratatui::{buffer::Buffer, layout::Rect, widgets::StatefulWidget};
use std::time::Instant;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = (0..3000)
        .map(|i| {
            format!("## Section {i}\n\n日本語 é 👩‍💻 with [a link](next.md) and **styled text**.\n\n")
        })
        .collect::<String>();
    let started = Instant::now();
    let mut markdown = MarkdownView::new(&source)?;
    let parse = started.elapsed();
    let started = Instant::now();
    let layout = markdown.prepare(100)?;
    let preparation = started.elapsed();
    let lines = layout.line_count();
    let positions = layout.links().as_ptr();
    let area = Rect::new(0, 0, 100, 30);
    let mut buffer = Buffer::empty(area);
    let started = Instant::now();
    for i in 0..2000 {
        buffer.reset();
        let layout = markdown.prepare(100)?;
        assert_eq!(
            layout.links().as_ptr(),
            positions,
            "scroll must reuse the layout"
        );
        let mut state = ViewState::default();
        state.scroll_to(markdown_ratatui::DocumentRow::new(i * 3));
        layout.widget().render(area, &mut buffer, &mut state);
        std::hint::black_box(&buffer);
    }
    println!(
        "bytes={} lines={lines} area=100x30 frames=2000 parse={parse:?} layout={preparation:?} scroll_total={:?}",
        source.len(),
        started.elapsed()
    );
    Ok(())
}
