use super::ffi;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(pub(super) ffi::GhosttyResult);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ghostty error {}", self.0)
    }
}

impl std::error::Error for Error {}

pub(super) trait GhosttyResultExt {
    fn into_result(self) -> Result<(), Error>;
}

impl GhosttyResultExt for ffi::GhosttyResult {
    fn into_result(self) -> Result<(), Error> {
        if self == ffi::GhosttyResult_GHOSTTY_SUCCESS {
            Ok(())
        } else {
            Err(Error(self))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    Clean,
    Partial,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalCompressionResult {
    Unsupported,
    Pending,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowSelection {
    pub start_x: u16,
    pub end_x: u16,
}

impl Dirty {
    pub(super) fn from_raw(value: ffi::GhosttyRenderStateDirty) -> Self {
        match value {
            ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE => Self::Clean,
            ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => Self::Partial,
            ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FULL => Self::Full,
            _ => Self::Full,
        }
    }

    pub(super) fn as_raw(self) -> ffi::GhosttyRenderStateDirty {
        match self {
            Self::Clean => ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE,
            Self::Partial => ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL,
            Self::Full => ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FULL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusEvent {
    Gained,
    Lost,
}

impl FocusEvent {
    pub(super) fn as_raw(self) -> ffi::GhosttyFocusEvent {
        match self {
            Self::Gained => ffi::GhosttyFocusEvent_GHOSTTY_FOCUS_GAINED,
            Self::Lost => ffi::GhosttyFocusEvent_GHOSTTY_FOCUS_LOST,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    pub(super) fn as_raw(self) -> ffi::GhosttyColorScheme {
        match self {
            Self::Light => ffi::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_LIGHT,
            Self::Dark => ffi::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_DARK,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorVisualStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
}

impl CursorVisualStyle {
    pub(super) fn from_raw(value: ffi::GhosttyRenderStateCursorVisualStyle) -> Self {
        match value {
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => {
                Self::Block
            }
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
                Self::Underline
            }
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
                Self::BlockHollow
            }
            _ => Self::Bar,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveScreen {
    Primary,
    Alternate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalScrollbar {
    pub total: usize,
    pub offset: usize,
    pub len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorViewport {
    pub x: u16,
    pub y: u16,
    pub wide_tail: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<ffi::GhosttyColorRgb> for RgbColor {
    fn from(value: ffi::GhosttyColorRgb) -> Self {
        Self {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}

pub fn default_palette() -> [RgbColor; 256] {
    let mut palette = [ffi::GhosttyColorRgb::default(); 256];
    unsafe { ffi::ghostty_color_palette_default(palette.as_mut_ptr()) };
    palette.map(Into::into)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellColor {
    Palette(u8),
    Rgb(RgbColor),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub fg_color: Option<CellColor>,
    pub bg_color: Option<CellColor>,
    pub underline_color: Option<CellColor>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: u8,
    pub underlined: bool,
}

impl From<ffi::GhosttyStyle> for CellStyle {
    fn from(value: ffi::GhosttyStyle) -> Self {
        Self {
            fg_color: cell_color_from_style_color(value.fg_color),
            bg_color: cell_color_from_style_color(value.bg_color),
            underline_color: cell_color_from_style_color(value.underline_color),
            bold: value.bold,
            italic: value.italic,
            faint: value.faint,
            blink: value.blink,
            inverse: value.inverse,
            invisible: value.invisible,
            strikethrough: value.strikethrough,
            overline: value.overline,
            underline: normalize_underline_style(value.underline),
            underlined: value.underline != 0,
        }
    }
}

fn normalize_underline_style(value: std::os::raw::c_int) -> u8 {
    match value {
        0..=5 => value as u8,
        _ => 1,
    }
}

fn cell_color_from_style_color(color: ffi::GhosttyStyleColor) -> Option<CellColor> {
    match color.tag {
        ffi::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_PALETTE => {
            // SAFETY: Ghostty's tagged union stores `palette` when the tag is PALETTE.
            Some(CellColor::Palette(unsafe { color.value.palette }))
        }
        ffi::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB => {
            // SAFETY: Ghostty's tagged union stores `rgb` when the tag is RGB.
            Some(CellColor::Rgb(unsafe { color.value.rgb }.into()))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderColors {
    pub background: RgbColor,
    pub foreground: RgbColor,
    pub palette: [RgbColor; 256],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellWide {
    Narrow,
    Wide,
    SpacerTail,
    SpacerHead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScreenTextCell {
    pub wide: CellWide,
    pub graphemes: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScreenTextRow {
    pub cells: Vec<ScreenTextCell>,
    pub soft_wrapped: bool,
    pub wrap_continuation: bool,
}

impl CellWide {
    pub(super) fn from_raw(value: ffi::GhosttyCellWide) -> Self {
        match value {
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW => Self::Narrow,
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_WIDE => Self::Wide,
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL => Self::SpacerTail,
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_HEAD => Self::SpacerHead,
            _ => Self::Narrow,
        }
    }
}
