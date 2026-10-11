# markdown-ratatui

`markdown-ratatui` renders Markdown inside any caller-owned ratatui area. It
accepts source text or a shared parsed `markdown-model` document, performs
Unicode display-width-aware layout, exposes typed heading and link positions,
and provides a `StatefulWidget` with caller-owned scroll and selection state.

The crate is a widget library, not a terminal application. It does not enable
raw mode, enter an alternate screen, read input, choose keys, open links,
access files or networks, start an async runtime, or terminate a process.

## Status and installation

The API is version 0.1 and follows Cargo SemVer conventions. Types re-exported
from `markdown-model` and types from `ratatui-core` are intentional parts of
the public contract.

Within this repository:

```toml
[dependencies]
markdown-ratatui = { path = "crates/markdown-ratatui" }
```

After a crates.io release:

```toml
[dependencies]
markdown-ratatui = "0.1"
```

No crates.io release is implied by this documentation; release preparation
must verify registry availability and ownership. The minimum supported Rust
version is 1.88. The crate has no Cargo features. Its normal dependency graph
uses `ratatui-core`, not a terminal backend or the complete `ratatui`
application crate.

## Render Markdown from memory

[`MarkdownView`] combines one shared document with one bounded layout cache.
[`MarkdownView::new`] parses source text once; [`MarkdownView::prepare`] lays
it out for a terminal width and reuses the result until the document, width, or
options change.

```rust
use markdown_ratatui::{MarkdownView, ViewState};
use ratatui_core::{
    buffer::Buffer,
    layout::Rect,
    widgets::StatefulWidget,
};

let mut markdown = MarkdownView::new("# Hello\n\n[Next](next.md)")?;
let area = Rect::new(2, 1, 30, 8);
let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 10));
let mut state = ViewState::default();

let layout = markdown.prepare(area.width)?;
layout.widget().render(area, &mut buffer, &mut state);

assert_eq!(layout.width(), 30);
assert_eq!(layout.headings()[0].heading_id().as_str(), "hello");
# Ok::<(), Box<dyn std::error::Error>>(())
```

Parsing and preparation are separate error boundaries:
[`MarkdownView::new`] returns [`ParseError`], while preparation returns
[`LayoutError`]. Both operations are synchronous and perform no I/O.

## Render an LSP hover or tooltip

Short-lived Markdown supplied by a language server needs no repository or Mira
application state. A host such as ChronoGit can retain one view and one state
inside its hover overlay:

```rust
use markdown_ratatui::{MarkdownView, ViewState};
use ratatui_core::{
    buffer::Buffer,
    layout::Rect,
    widgets::StatefulWidget,
};

let hover_markdown = "`Action` runs the selected **operation**.";
let mut hover = MarkdownView::new(hover_markdown)?;
let mut hover_state = ViewState::default();
let popup = Rect::new(4, 2, 48, 10);
let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 16));

hover
    .prepare(popup.width)?
    .widget()
    .render(popup, &mut buffer, &mut hover_state);

hover_state.scroll_forward(1); // The host owns the key binding.
# Ok::<(), Box<dyn std::error::Error>>(())
```

Replacing hover content with [`MarkdownView::set_document`] invalidates only
the cached layout. Resetting or retaining [`ViewState`] is an application
decision.

## Reuse a parsed document

`markdown-ratatui` re-exports [`Document`] and the navigation identity types,
so a consumer needs only one direct dependency for ordinary embedding:

```rust
use std::sync::Arc;
use markdown_ratatui::{Document, MarkdownView};

let document = Arc::new(Document::parse("# Shared\n\nA cached document.")?);
let mut first = MarkdownView::from_document(document.clone());
let mut second = MarkdownView::from_document(document);

assert_eq!(first.prepare(40)?.headings()[0].heading_id().as_str(), "shared");
assert_eq!(second.prepare(80)?.headings()[0].heading_id().as_str(), "shared");
# Ok::<(), Box<dyn std::error::Error>>(())
```

[`MarkdownView::document`] borrows the document. Use
[`MarkdownView::shared_document`] only when the caller needs the exact `Arc`.
Supplying an already parsed document never reparses it.

## Caller-owned navigation state

[`ViewState`] keeps its fields private and exposes intention-revealing methods.
The host decides which keys scroll, select links, and activate destinations:

```rust
use markdown_ratatui::{MarkdownView, ViewState};

let mut markdown = MarkdownView::new("[one](one.md) and [two](two.md)")?;
let mut state = ViewState::default();
let first_link = markdown.document().links()[0].id();

state.scroll_forward(1);
state.select_link(Some(first_link));

let layout = markdown.prepare(20)?;
if let Some(position) = layout
    .links()
    .iter()
    .find(|position| position.link_id() == first_link)
{
    state.reveal(position.row(), 8);
}
state.clamp(layout, 8);

assert_eq!(markdown.document().link(first_link)
    .expect("the ID came from this document")
    .destination().as_str(), "one.md");
# Ok::<(), Box<dyn std::error::Error>>(())
```

Selection changes style only. The widget never emits OSC 8 links or opens a
browser. Destination classification, filesystem containment, and explicit user
intent belong to the host application.

## Width, positions, and policy

Call [`MarkdownView::prepare`] with the width of the exact area passed to the
widget. The widget clips if rendered into a narrower area; it does not rewrap.
Width zero produces an empty layout. A grapheme that cannot fit is replaced by
`�` rather than split across terminal cells.

[`Layout::links`] uses [`LinkPosition`] values containing a typed [`LinkId`],
[`DocumentRow`], and [`CellRange`]. Wrapping or an inline style boundary can
produce several positions for one link. [`Layout::headings`] uses the same
[`HeadingId`] generated by `markdown-model`, allowing HTML and terminal views
to share anchor identities.

[`LayoutOptions`] contains a [`Theme`], [`CodePolicy`], [`TablePolicy`], and a
validated [`LineLimit`]. `LineLimit::new(0)` intentionally rejects non-empty
layouts; values above [`LineLimit::MAX`] are rejected during construction.

```rust
use markdown_ratatui::{
    CodePolicy, LayoutOptions, LineLimit, MarkdownView, TablePolicy, Theme,
};
use ratatui_core::style::{Color, Style};

let mut markdown = MarkdownView::new("```text\na very long line\n```")?;
markdown.set_options(LayoutOptions {
    theme: Theme {
        heading: Style::default().fg(Color::Magenta),
        ..Theme::default()
    },
    code: CodePolicy::Clip,
    tables: TablePolicy::Stacked,
    line_limit: LineLimit::new(10_000)?,
});
assert_eq!(markdown.prepare(20)?.width(), 20);
# Ok::<(), Box<dyn std::error::Error>>(())
```

[`sanitize_terminal_text`] removes control characters, preserves line feeds,
and expands tabs. Layout applies it automatically to model text; hosts can use
it for untrusted titles and status strings rendered outside the widget.

## Complete example

The repository includes a complete host that owns its terminal, panels, event
loop, keys, layout, and state:

```console
cargo run -p markdown-ratatui --example embedded -- --buffer
```

The interactive form is `cargo run -p markdown-ratatui --example embedded`.

## License

Licensed under either the
[Apache License, Version 2.0](https://github.com/karanabe/mira/blob/master/LICENSE-APACHE)
or the [MIT license](https://github.com/karanabe/mira/blob/master/LICENSE-MIT) at
your option.

[`CellRange`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.CellRange.html
[`CodePolicy`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/enum.CodePolicy.html
[`Document`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.Document.html
[`DocumentRow`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.DocumentRow.html
[`HeadingId`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.HeadingId.html
[`Layout::headings`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.Layout.html#method.headings
[`Layout::links`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.Layout.html#method.links
[`LayoutError`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/enum.LayoutError.html
[`LayoutOptions`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.LayoutOptions.html
[`LineLimit`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.LineLimit.html
[`LineLimit::MAX`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.LineLimit.html#associatedconstant.MAX
[`LinkId`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.LinkId.html
[`LinkPosition`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.LinkPosition.html
[`MarkdownView`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.MarkdownView.html
[`MarkdownView::document`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.MarkdownView.html#method.document
[`MarkdownView::new`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.MarkdownView.html#method.new
[`MarkdownView::prepare`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.MarkdownView.html#method.prepare
[`MarkdownView::set_document`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.MarkdownView.html#method.set_document
[`MarkdownView::shared_document`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.MarkdownView.html#method.shared_document
[`ParseError`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/enum.ParseError.html
[`TablePolicy`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/enum.TablePolicy.html
[`Theme`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.Theme.html
[`ViewState`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/struct.ViewState.html
[`sanitize_terminal_text`]: https://docs.rs/markdown-ratatui/latest/markdown_ratatui/fn.sanitize_terminal_text.html
