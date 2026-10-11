use markdown_model::Document;
use markdown_ratatui::{
    Layout, LayoutError, LayoutOptions, LineLimit, MarkdownView, TablePolicy, ViewState,
};
use ratatui_core::{buffer::Buffer, layout::Rect, widgets::StatefulWidget};
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;
#[test]
fn unicode_long_lines_tables_and_zero_width_are_bounded() {
    let doc = Document::parse(&format!("# 日本語\n\né 👩‍💻 🇯🇵 🙂 全角ＡＢＣ [long URL](https://example.com)\n\n{}\n\n```rust\n{}\n```\n\n| One | Two |\n| :- | -: |\n| 日本語 | 👩‍💻 |", "https://example.com/".repeat(30), "長いコード".repeat(40))).unwrap();
    for width in [0, 1, 2, 3, 8, 20, 80] {
        let layout = Layout::new(&doc, width, &LayoutOptions::default()).unwrap();
        for line in layout.plain_lines() {
            assert!(
                line.width() <= usize::from(width),
                "width {width}: {line:?}"
            );
        }
        let area = Rect::new(3, 2, width, 12);
        let mut buf = Buffer::empty(Rect::new(0, 0, 90, 20));
        layout
            .widget()
            .render(area, &mut buf, &mut ViewState::default());
        assert_eq!(buf[(0, 0)].symbol(), " ");
        if width == 0 {
            assert_eq!(layout.line_count(), 0);
        }
    }
    let layout = Layout::new(&doc, 80, &LayoutOptions::default()).unwrap();
    let text = layout.plain_lines().join("\n");
    for sample in ["é", "👩‍💻", "🇯🇵", "日本語"] {
        assert!(text.contains(sample));
    }
}
#[test]
fn wrapped_link_cells_match_rendered_scrolled_buffer() {
    let doc = Document::parse("# Heading\n\n[abcdefghijklmnop](next.md)\n\ntrailer\n\nmore\n\nend")
        .unwrap();
    let layout = Layout::new(&doc, 5, &LayoutOptions::default()).unwrap();
    let link_id = doc.links()[0].id();
    let positions: Vec<_> = layout
        .links()
        .iter()
        .filter(|position| position.link_id() == link_id)
        .collect();
    assert!(positions.len() >= 4);
    for p in positions {
        let mut state = ViewState::default();
        state.scroll_to(p.row());
        state.select_link(Some(link_id));
        let mut buffer = Buffer::empty(Rect::new(0, 0, 5, 1));
        layout.widget().render(buffer.area, &mut buffer, &mut state);
        let line = &layout.plain_lines()[p.row().index()];
        let rendered: String = (0..5).map(|x| buffer[(x, 0)].symbol()).collect();
        assert_eq!(rendered.trim_end(), line.trim_end());
        for x in p.columns().range() {
            assert_eq!(
                buffer[(x, 0)].bg,
                LayoutOptions::default().theme.selected.bg.unwrap()
            );
        }
    }
    assert_eq!(layout.headings()[0].heading_id(), doc.headings()[0].id());
}
#[test]
fn cache_invalidates_only_for_document_width_or_options() {
    let original = Arc::new(Document::parse("# old\n\n[hello](next.md)").unwrap());
    let mut markdown = MarkdownView::from_document(original.clone());
    let first_lines = markdown.prepare(10).unwrap().plain_lines();
    let first_ptr = markdown.prepare(10).unwrap().links().as_ptr();
    for scroll in 0..100 {
        let layout = markdown.prepare(10).unwrap();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 2));
        let mut state = ViewState::default();
        state.scroll_to(markdown_ratatui::DocumentRow::new(scroll));
        layout.widget().render(buffer.area, &mut buffer, &mut state);
        assert_eq!(layout.links().as_ptr(), first_ptr);
    }
    assert!(Arc::ptr_eq(markdown.shared_document(), &original));
    assert_ne!(markdown.prepare(2).unwrap().plain_lines(), first_lines);
    markdown.set_document(Arc::new(Document::parse("# new").unwrap()));
    assert_eq!(
        markdown.prepare(10).unwrap().headings()[0]
            .heading_id()
            .as_str(),
        "new"
    );
    let options = LayoutOptions {
        line_limit: LineLimit::new(0).unwrap(),
        ..LayoutOptions::default()
    };
    markdown.set_options(options.clone());
    assert_eq!(markdown.options(), &options);
    assert!(matches!(
        markdown.prepare(10),
        Err(LayoutError::TooManyLines { .. })
    ));
    assert!(LineLimit::new(LineLimit::MAX + 1).is_err());
}
#[test]
fn table_alignment_and_narrow_fallback_keep_content() {
    let doc = Document::parse("| L | R |\n| :- | -: |\n| xxxx | z |").unwrap();
    let wide = Layout::new(&doc, 40, &LayoutOptions::default())
        .unwrap()
        .plain_lines()
        .join("\n");
    assert!(wide.contains("L    │ R"));
    let stacked = Layout::new(
        &doc,
        40,
        &LayoutOptions {
            tables: TablePolicy::Stacked,
            ..LayoutOptions::default()
        },
    )
    .unwrap()
    .plain_lines()
    .join("\n");
    assert!(stacked.contains("L: xxxx"));
    assert!(stacked.contains("R: z"));
}
#[test]
fn shrink_clamps_scroll_and_control_bytes_never_reach_buffer() {
    let doc = Document::parse("# hi\n\n\x1b[2Jcontrol\u{009b}\x07").unwrap();
    let layout = Layout::new(&doc, 30, &LayoutOptions::default()).unwrap();
    let mut state = ViewState::default();
    state.scroll_to_end();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 30, 20));
    layout.widget().render(buffer.area, &mut buffer, &mut state);
    assert_eq!(state.scroll().index(), 0);
    for cell in buffer.content() {
        assert!(!cell.symbol().chars().any(char::is_control));
    }
}

#[test]
fn mermaid_fences_stay_readable_in_terminal_layout() {
    let doc = Document::parse("```mermaid\nflowchart LR\n  Markdown --> Mira\n```").unwrap();
    let text = Layout::new(&doc, 40, &LayoutOptions::default())
        .unwrap()
        .plain_lines()
        .join("\n");
    assert!(text.contains("┌ mermaid"));
    assert!(text.contains("flowchart LR"));
    assert!(text.contains("Markdown --> Mira"));
}
