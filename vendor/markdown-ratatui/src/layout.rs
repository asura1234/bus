use crate::{MarkdownWidget, sanitize_terminal_text};
use markdown_model::{
    Alignment, Block, BlockKind, Document, HeadingId, Inline, InlineKind, LinkId, ListKind, Table,
    TableCell, TableRow, TaskState, plain_text,
};
use ratatui_core::style::{Color, Modifier, Style};
use std::{fmt, ops::Range};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Caller-provided styles used while preparing and rendering a layout.
///
/// Inline emphasis and selection are combined with these values using
/// [`Style::patch`]. The crate never installs a global theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Ordinary text.
    pub text: Style,
    /// Heading text.
    pub heading: Style,
    /// Link labels.
    pub link: Style,
    /// Code and inline code.
    pub code: Style,
    /// Quote/list prefixes and table separators.
    pub muted: Style,
    /// Applied to the selected link by the widget.
    pub selected: Style,
}
impl Default for Theme {
    fn default() -> Self {
        Self {
            text: Style::default(),
            heading: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            link: Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::UNDERLINED),
            code: Style::default().fg(Color::Yellow),
            muted: Style::default().fg(Color::DarkGray),
            selected: Style::default().bg(Color::Blue).fg(Color::White),
        }
    }
}
/// Controls long code-line behavior during layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CodePolicy {
    /// Wrap long code lines at terminal cell boundaries.
    #[default]
    Wrap,
    /// Clip long lines to the available width without horizontal scrolling.
    Clip,
}
/// Controls table presentation during layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TablePolicy {
    /// Use aligned columns if they fit, otherwise stack labeled cells.
    #[default]
    Auto,
    /// Always stack each row's labeled cells.
    Stacked,
}
/// A validated maximum number of retained layout rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LineLimit(usize);
impl LineLimit {
    /// The largest supported retained-row budget.
    pub const MAX: usize = 200_000;
    /// The default retained-row budget.
    pub const DEFAULT: Self = Self(100_000);

    /// Creates a retained-row budget. Zero rejects every non-empty layout.
    ///
    /// # Errors
    /// Returns [`LineLimitError`] when `lines` exceeds [`LineLimit::MAX`].
    pub const fn new(lines: usize) -> Result<Self, LineLimitError> {
        if lines <= Self::MAX {
            Ok(Self(lines))
        } else {
            Err(LineLimitError { requested: lines })
        }
    }

    /// Returns the maximum retained row count.
    pub const fn get(self) -> usize {
        self.0
    }
}
impl Default for LineLimit {
    fn default() -> Self {
        Self::DEFAULT
    }
}
impl fmt::Display for LineLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
/// An attempted line limit exceeded [`LineLimit::MAX`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineLimitError {
    requested: usize,
}
impl LineLimitError {
    /// Returns the rejected row count.
    pub const fn requested(self) -> usize {
        self.requested
    }
}
impl fmt::Display for LineLimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line limit {} exceeds the maximum of {}",
            self.requested,
            LineLimit::MAX
        )
    }
}
impl std::error::Error for LineLimitError {}

/// Appearance, wrapping, and resource policy for a [`Layout`].
///
/// [`crate::MarkdownView`] compares the complete value when deciding whether
/// its single cached layout can be reused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutOptions {
    /// Styles applied during layout and link selection.
    pub theme: Theme,
    /// Code line wrapping.
    pub code: CodePolicy,
    /// Table layout and narrow-width fallback.
    pub tables: TablePolicy,
    /// Maximum retained rows.
    pub line_limit: LineLimit,
}
impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            code: CodePolicy::Wrap,
            tables: TablePolicy::Auto,
            line_limit: LineLimit::default(),
        }
    }
}
/// An error produced while preparing terminal rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// Prepared text would exceed the row budget.
    TooManyLines {
        /// Configured retained-row limit.
        limit: LineLimit,
    },
}
impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyLines { limit } => write!(
                f,
                "layout exceeds {limit} lines; use a wider area or a smaller document"
            ),
        }
    }
}
impl std::error::Error for LayoutError {}

/// The terminal cells occupied by one styled run of a link.
///
/// Wrapping always produces multiple entries. A style change inside a link can
/// also produce adjacent entries on the same row with the same ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkPosition {
    id: LinkId,
    row: DocumentRow,
    columns: CellRange,
}
impl LinkPosition {
    /// Returns the identity from [`markdown_model::Document::links`].
    pub const fn link_id(&self) -> LinkId {
        self.id
    }
    /// Returns the zero-based document row before scrolling.
    pub const fn row(&self) -> DocumentRow {
        self.row
    }
    /// Returns the half-open terminal-cell columns within the row.
    pub const fn columns(&self) -> CellRange {
        self.columns
    }
}
/// The document row at which a shared heading anchor starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingPosition {
    id: HeadingId,
    row: DocumentRow,
}
impl HeadingPosition {
    /// Returns the anchor ID from [`markdown_model::Document::headings`].
    pub const fn heading_id(&self) -> &HeadingId {
        &self.id
    }
    /// Returns the zero-based row before scrolling.
    pub const fn row(&self) -> DocumentRow {
        self.row
    }
}
/// A zero-based row in a prepared Markdown document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentRow(usize);
impl DocumentRow {
    /// Creates a document-relative row.
    pub const fn new(index: usize) -> Self {
        Self(index)
    }
    /// Returns the zero-based row index.
    pub const fn index(self) -> usize {
        self.0
    }
}
/// A half-open range of terminal cells within one prepared row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellRange {
    start: u16,
    end: u16,
}
impl CellRange {
    fn new(start: u16, end: u16) -> Self {
        debug_assert!(start <= end);
        Self { start, end }
    }
    /// Returns the inclusive starting cell.
    pub const fn start(self) -> u16 {
        self.start
    }
    /// Returns the exclusive ending cell.
    pub const fn end(self) -> u16 {
        self.end
    }
    /// Returns the number of terminal cells in the range.
    pub const fn len(self) -> u16 {
        self.end - self.start
    }
    /// Returns whether the range contains no terminal cells.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
    /// Returns an ordinary cell range.
    pub const fn range(self) -> Range<u16> {
        self.start..self.end
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Run {
    pub text: String,
    pub style: Style,
    pub link: Option<LinkId>,
}
/// Prepared terminal rows and document-relative navigation positions.
///
/// A layout owns its rendered text and styles, so the source [`Document`] and
/// [`LayoutOptions`] can be dropped after construction. Rendering never wraps or
/// reparses it. Prepare a new layout when the content-area width changes.
#[derive(Clone, Debug)]
pub struct Layout {
    width: u16,
    pub(crate) lines: Vec<Vec<Run>>,
    links: Vec<LinkPosition>,
    headings: Vec<HeadingPosition>,
    pub(crate) selected: Style,
}
impl Layout {
    /// Lays out a parsed document synchronously for `width` terminal cells.
    ///
    /// Unicode grapheme clusters are measured by display width. Width zero
    /// produces an empty layout. A grapheme wider than the available content
    /// width becomes `�`; a wide grapheme is never split across cells.
    ///
    /// # Errors
    /// Returns [`LayoutError::TooManyLines`] before retaining more than the
    /// configured [`LayoutOptions::line_limit`] budget.
    pub fn new(
        document: &Document,
        width: u16,
        options: &LayoutOptions,
    ) -> Result<Self, LayoutError> {
        let mut builder = Builder {
            layout: Self {
                width,
                lines: Vec::new(),
                links: Vec::new(),
                headings: Vec::new(),
                selected: options.theme.selected,
            },
            options,
        };
        if width != 0 {
            builder.blocks(document.blocks(), "")?;
        }
        let mut links = Vec::new();
        for (row, runs) in builder.layout.lines.iter().enumerate() {
            let mut column = 0;
            for run in runs {
                let end = column + UnicodeWidthStr::width(run.text.as_str()) as u16;
                if let Some(id) = run.link {
                    links.push(LinkPosition {
                        id,
                        row: DocumentRow::new(row),
                        columns: CellRange::new(column, end),
                    });
                }
                column = end;
            }
        }
        builder.layout.links = links;
        Ok(builder.layout)
    }
    /// Width in terminal cells for which this layout was prepared.
    pub fn width(&self) -> u16 {
        self.width
    }
    /// Total row count after wrapping, including block spacing.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
    /// Returns document-relative link cells after wrapping.
    ///
    /// Positions use the IDs assigned by [`markdown_model::Document::links`].
    /// Widget offsets and scroll state are not included.
    pub fn links(&self) -> &[LinkPosition] {
        &self.links
    }
    /// Returns heading anchors and their document-relative rows.
    pub fn headings(&self) -> &[HeadingPosition] {
        &self.headings
    }
    /// Allocates the plain text of every prepared row.
    ///
    /// This is useful for tests, export, and accessibility adapters. Render
    /// [`Layout::widget`] when styles are needed or the allocation is unwanted.
    pub fn plain_lines(&self) -> Vec<String> {
        self.lines
            .iter()
            .map(|line| line.iter().map(|run| run.text.as_str()).collect())
            .collect()
    }
    /// Borrows this layout as a [`MarkdownWidget`].
    pub fn widget(&self) -> MarkdownWidget<'_> {
        MarkdownWidget { layout: self }
    }
}
struct Builder<'a> {
    layout: Layout,
    options: &'a LayoutOptions,
}
fn run(text: impl Into<String>, style: Style) -> Run {
    Run {
        text: text.into(),
        style,
        link: None,
    }
}
fn push_run(line: &mut Vec<Run>, value: Run) {
    if let Some(last) = line.last_mut()
        && last.style == value.style
        && last.link == value.link
    {
        last.text.push_str(&value.text);
        return;
    }
    line.push(value);
}
impl Builder<'_> {
    fn push(&mut self, line: Vec<Run>) -> Result<(), LayoutError> {
        let limit = self.options.line_limit;
        if self.layout.lines.len() >= limit.get() {
            return Err(LayoutError::TooManyLines { limit });
        }
        self.layout.lines.push(line);
        Ok(())
    }
    fn text(
        &mut self,
        runs: Vec<Run>,
        first: &str,
        continuation: &str,
        clip: bool,
    ) -> Result<(), LayoutError> {
        let width = usize::from(self.layout.width);
        let prefix = |p: &str| -> String {
            let mut out = String::new();
            for g in p.graphemes(true) {
                if out.width() + g.width() > width.saturating_sub(1) {
                    break;
                }
                out.push_str(g);
            }
            out
        };
        let first = prefix(first);
        let continuation = prefix(continuation);
        let mut line = vec![run(first.clone(), self.options.theme.muted)];
        let mut used = first.width();
        let mut skipped = false;
        for segment in runs {
            let text = sanitize_terminal_text(&segment.text);
            for word in text.split_word_bounds() {
                let word_width = word.width();
                if !clip
                    && !word.contains('\n')
                    && word_width <= width.saturating_sub(continuation.width())
                    && used > first.width()
                    && used + word_width > width
                {
                    self.push(std::mem::take(&mut line))?;
                    line.push(run(continuation.clone(), self.options.theme.muted));
                    used = continuation.width();
                }
                for g in word.graphemes(true) {
                    if g == "\n" {
                        self.push(std::mem::take(&mut line))?;
                        line.push(run(continuation.clone(), self.options.theme.muted));
                        used = continuation.width();
                        skipped = false;
                        continue;
                    }
                    if skipped {
                        continue;
                    }
                    let mut g = g;
                    let mut cells = g.width();
                    let available = width.saturating_sub(continuation.width());
                    if cells > available {
                        g = "�";
                        cells = 1;
                    }
                    if used + cells > width {
                        if clip {
                            skipped = true;
                            continue;
                        }
                        self.push(std::mem::take(&mut line))?;
                        line.push(run(continuation.clone(), self.options.theme.muted));
                        used = continuation.width();
                    }
                    push_run(
                        &mut line,
                        Run {
                            text: g.to_owned(),
                            style: segment.style,
                            link: segment.link,
                        },
                    );
                    used += cells;
                }
            }
        }
        self.push(line)
    }
    fn blocks(&mut self, blocks: &[Block], prefix: &str) -> Result<(), LayoutError> {
        let theme = &self.options.theme;
        for block in blocks {
            match block.kind() {
                BlockKind::Paragraph(content) => self.text(
                    inlines(content, theme.text, theme, None),
                    prefix,
                    prefix,
                    false,
                )?,
                BlockKind::Heading { id, content, .. } => {
                    self.layout.headings.push(HeadingPosition {
                        id: id.clone(),
                        row: DocumentRow::new(self.layout.lines.len()),
                    });
                    self.text(
                        inlines(content, theme.heading, theme, None),
                        prefix,
                        prefix,
                        false,
                    )?;
                }
                BlockKind::Quote(children) => self.blocks(children, &format!("{prefix}│ "))?,
                BlockKind::List { kind, items } => {
                    for (index, item) in items.iter().enumerate() {
                        let marker = match item.task() {
                            TaskState::Checked => "[x] ".to_owned(),
                            TaskState::Unchecked => "[ ] ".to_owned(),
                            TaskState::Plain => match kind {
                                ListKind::Unordered => "• ".to_owned(),
                                ListKind::Ordered { start } => {
                                    format!("{}. ", start.saturating_add(index as u64))
                                }
                            },
                        };
                        let first = format!("{prefix}{marker}");
                        let rest = format!("{prefix}{}", " ".repeat(marker.width()));
                        if let Some((head, tail)) = item.blocks().split_first() {
                            if let BlockKind::Paragraph(content) = head.kind() {
                                self.text(
                                    inlines(content, theme.text, theme, None),
                                    &first,
                                    &rest,
                                    false,
                                )?;
                                self.blocks(tail, &rest)?;
                            } else {
                                self.text(Vec::new(), &first, &rest, false)?;
                                self.blocks(item.blocks(), &rest)?;
                            }
                        } else {
                            self.text(Vec::new(), &first, &rest, false)?;
                        }
                    }
                }
                BlockKind::Code { kind, text } => {
                    let info = kind.info().unwrap_or_default();
                    self.text(
                        vec![run(
                            format!("┌ {}", sanitize_terminal_text(info)),
                            theme.muted,
                        )],
                        prefix,
                        prefix,
                        false,
                    )?;
                    for line in text.lines() {
                        self.text(
                            vec![run(line, theme.code)],
                            &format!("{prefix}│ "),
                            &format!("{prefix}│ "),
                            self.options.code == CodePolicy::Clip,
                        )?;
                    }
                    self.text(vec![run("└", theme.muted)], prefix, prefix, false)?;
                }
                BlockKind::Literal(text) => {
                    self.text(vec![run(text, theme.text)], prefix, prefix, false)?
                }
                BlockKind::Rule => self.text(
                    vec![run(
                        "─".repeat(usize::from(self.layout.width).saturating_sub(prefix.width())),
                        theme.muted,
                    )],
                    prefix,
                    prefix,
                    false,
                )?,
                BlockKind::Table(table) => self.table(table, prefix)?,
            }
            self.push(Vec::new())?;
        }
        Ok(())
    }
    fn table(&mut self, table: &Table, prefix: &str) -> Result<(), LayoutError> {
        let theme = &self.options.theme;
        let header = table.header().cells();
        let rows = table.rows();
        let align = table.alignments();
        let mut widths: Vec<usize> = header
            .iter()
            .map(|c| {
                sanitize_terminal_text(&plain_text(c.inlines()))
                    .width()
                    .max(1)
            })
            .collect();
        for row in rows {
            for (i, cell) in row.cells().iter().enumerate() {
                if let Some(w) = widths.get_mut(i) {
                    *w = (*w).max(sanitize_terminal_text(&plain_text(cell.inlines())).width());
                }
            }
        }
        let total =
            widths.iter().sum::<usize>() + widths.len().saturating_sub(1) * 3 + prefix.width();
        if total > usize::from(self.layout.width) || self.options.tables == TablePolicy::Stacked {
            // Keep headers even in an empty table.
            self.text(
                inlines_join(header, theme.heading, theme),
                prefix,
                prefix,
                false,
            )?;
            for row in rows {
                for (i, cell) in row.cells().iter().enumerate() {
                    let label = header
                        .get(i)
                        .map_or_else(|| format!("Column {}", i + 1), |c| plain_text(c.inlines()));
                    let mut runs = vec![run(format!("{label}: "), theme.heading)];
                    runs.extend(inlines(cell.inlines(), theme.text, theme, None));
                    self.text(runs, prefix, prefix, false)?;
                }
                self.push(Vec::new())?;
            }
        } else {
            for (index, row) in std::iter::once(header)
                .chain(rows.iter().map(TableRow::cells))
                .enumerate()
            {
                let mut runs = Vec::new();
                for (i, cell) in row.iter().enumerate() {
                    if i > 0 {
                        runs.push(run(" │ ", theme.muted));
                    }
                    let content = inlines(
                        cell.inlines(),
                        if index == 0 {
                            theme.heading
                        } else {
                            theme.text
                        },
                        theme,
                        None,
                    );
                    let size: usize = content
                        .iter()
                        .map(|run| sanitize_terminal_text(&run.text).width())
                        .sum();
                    let pad = widths.get(i).copied().unwrap_or(size).saturating_sub(size);
                    let left = match align.get(i).unwrap_or(&Alignment::None) {
                        Alignment::Right => pad,
                        Alignment::Center => pad / 2,
                        _ => 0,
                    };
                    runs.push(run(" ".repeat(left), theme.text));
                    runs.extend(content);
                    runs.push(run(" ".repeat(pad - left), theme.text));
                }
                self.text(runs, prefix, prefix, false)?;
                if index == 0 {
                    self.text(
                        vec![run(
                            widths
                                .iter()
                                .map(|w| "─".repeat(*w))
                                .collect::<Vec<_>>()
                                .join("─┼─"),
                            theme.muted,
                        )],
                        prefix,
                        prefix,
                        false,
                    )?;
                }
            }
        }
        Ok(())
    }
}
fn inlines_join(cells: &[TableCell], style: Style, theme: &Theme) -> Vec<Run> {
    let mut runs = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            runs.push(run(" / ", theme.muted));
        }
        runs.extend(inlines(cell.inlines(), style, theme, None));
    }
    runs
}
fn inlines(content: &[Inline], style: Style, theme: &Theme, link: Option<LinkId>) -> Vec<Run> {
    let mut out = Vec::new();
    for inline in content {
        let (text, style) = match inline.kind() {
            InlineKind::Text(s) => (s.clone(), style),
            InlineKind::Code(s) => (s.clone(), style.patch(theme.code)),
            InlineKind::SoftBreak => (" ".to_owned(), style),
            InlineKind::HardBreak => ("\n".to_owned(), style),
            InlineKind::Emphasis(c) => {
                out.extend(inlines(
                    c,
                    style.add_modifier(Modifier::ITALIC),
                    theme,
                    link,
                ));
                continue;
            }
            InlineKind::Strong(c) => {
                out.extend(inlines(c, style.add_modifier(Modifier::BOLD), theme, link));
                continue;
            }
            InlineKind::Strikethrough(c) => {
                out.extend(inlines(
                    c,
                    style.add_modifier(Modifier::CROSSED_OUT),
                    theme,
                    link,
                ));
                continue;
            }
            InlineKind::Link { id, content } => {
                let children = if content.is_empty() {
                    vec![run("[link]", theme.link)]
                } else {
                    inlines(content, style.patch(theme.link), theme, Some(*id))
                };
                out.extend(children.into_iter().map(|mut r| {
                    r.link = Some(*id);
                    r
                }));
                continue;
            }
            InlineKind::Image { alt, .. } => (
                format!("[image: {}]", plain_text(alt)),
                style.patch(theme.muted),
            ),
        };
        out.push(Run { text, style, link });
    }
    out
}
