#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

mod layout;
mod text;
mod widget;

pub use layout::{
    CellRange, CodePolicy, DocumentRow, HeadingPosition, Layout, LayoutError, LayoutOptions,
    LineLimit, LineLimitError, LinkPosition, TablePolicy, Theme,
};
pub use markdown_model::{Document, HeadingId, LinkId, ParseError};
use std::sync::Arc;
pub use text::sanitize_terminal_text;
pub use widget::{MarkdownWidget, ViewState};

/// A parsed document with a single width-dependent layout cache.
///
/// Construction parses at most once. [`MarkdownView::prepare`] retains one
/// layout; changing the document, width, or options invalidates it. Scrolling
/// and link selection live in [`ViewState`] and never parse or lay out content.
#[derive(Debug)]
pub struct MarkdownView {
    document: Arc<Document>,
    options: LayoutOptions,
    cached: Option<Layout>,
}
impl MarkdownView {
    /// Parses an in-memory string with [`markdown_model::ParseLimits::default`].
    ///
    /// The resulting document owns its content. This method is synchronous and
    /// performs no I/O.
    ///
    /// # Errors
    /// Returns [`ParseError`] if the source exceeds a parser resource limit.
    /// Layout errors cannot occur until [`MarkdownView::prepare`] is called.
    pub fn new(source: &str) -> Result<Self, ParseError> {
        Ok(Self::from_document(Arc::new(Document::parse(source)?)))
    }
    /// Uses an existing shared document without parsing or I/O.
    pub fn from_document(document: Arc<Document>) -> Self {
        Self {
            document,
            options: LayoutOptions::default(),
            cached: None,
        }
    }
    /// Returns the document from which headings and links originate.
    pub fn document(&self) -> &Document {
        &self.document
    }
    /// Returns the exact shared document retained by this view.
    pub const fn shared_document(&self) -> &Arc<Document> {
        &self.document
    }
    /// Returns the current appearance and wrapping policy.
    pub const fn options(&self) -> &LayoutOptions {
        &self.options
    }
    /// Replaces the document and unconditionally discards the cached layout.
    pub fn set_document(&mut self, document: Arc<Document>) {
        self.document = document;
        self.cached = None;
    }
    /// Replaces appearance and wrapping policy.
    ///
    /// A value equal to the current options preserves the cached layout;
    /// different options discard it.
    pub fn set_options(&mut self, options: LayoutOptions) {
        if self.options != options {
            self.options = options;
            self.cached = None;
        }
    }
    /// Prepares or reuses the cached layout for `width` terminal cells.
    ///
    /// Repeated calls with the same document, options, and width return the
    /// same retained layout. A different width replaces the previous entry.
    ///
    /// # Errors
    /// Returns [`LayoutError::TooManyLines`] if preparation would exceed the
    /// configured line budget.
    pub fn prepare(&mut self, width: u16) -> Result<&Layout, LayoutError> {
        if self
            .cached
            .as_ref()
            .is_none_or(|layout| layout.width() != width)
        {
            self.cached = Some(Layout::new(&self.document, width, &self.options)?);
        }
        Ok(self.cached.as_ref().expect("layout was prepared above"))
    }
}
