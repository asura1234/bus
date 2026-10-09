//! Safe VT facade. The exported paths remain stable while handles and protocol owners split.
#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all,
    rustdoc::all
)]
pub mod ffi;

mod callbacks;
mod consts;
mod input;
mod kitty;
mod kitty_placement;
mod read;
mod render;
mod terminal;
mod types;

pub(crate) use consts::KITTY_UNICODE_PLACEHOLDER;
#[cfg(test)]
pub use consts::MODE_GRAPHEME_CLUSTER;
pub use consts::{
    MODE_APPLICATION_CURSOR_KEYS, MODE_BRACKETED_PASTE, MODE_COLOR_SCHEME_REPORT, MODE_FOCUS_EVENT,
    MODE_MOUSE_ALTERNATE_SCROLL, MODE_MOUSE_SGR_PIXELS, MODE_SYNCHRONIZED_OUTPUT, MOD_ALT,
    MOD_CTRL, MOD_SHIFT, MOD_SUPER, MOUSE_ACTION_MOTION, MOUSE_ACTION_PRESS, MOUSE_ACTION_RELEASE,
    MOUSE_BUTTON_LEFT, MOUSE_BUTTON_MIDDLE, MOUSE_BUTTON_RIGHT, MOUSE_BUTTON_WHEEL_DOWN,
    MOUSE_BUTTON_WHEEL_LEFT, MOUSE_BUTTON_WHEEL_RIGHT, MOUSE_BUTTON_WHEEL_UP, MOUSE_FORMAT_SGR,
    MOUSE_FORMAT_SGR_PIXELS,
};
pub use input::{encode_focus, KeyEncoder, KeyEvent, MouseEncoder, MouseEvent};
pub use kitty::{KittyImageDescriptor, KittyImagePlacement, KittyPlacementRenderInfo};
pub use read::{CellBasicData, RowCellIter, RowCells, RowIter, RowIterator};
pub use render::RenderState;
pub use terminal::Terminal;
use types::GhosttyResultExt;
pub use types::{
    default_palette, ActiveScreen, CellColor, CellStyle, CellWide, ColorScheme, CursorViewport,
    CursorVisualStyle, Dirty, Error, FocusEvent, RenderColors, RgbColor, RowSelection,
    TerminalScrollbar,
};
pub(crate) use types::{ScreenTextCell, ScreenTextRow, TerminalCompressionResult};

#[cfg(test)]
mod tests {
    use super::callbacks::{clipboard_write_trampoline, TerminalCallbackState};
    use super::kitty::kitty_image_fingerprint;
    use super::*;
    use crate::utils::text::width::unicode_codepoint_width;

    #[path = "input_test.rs"]
    mod input;
    #[path = "kitty_test.rs"]
    mod kitty;
    #[path = "render_test.rs"]
    mod render;
    #[path = "terminal_test.rs"]
    mod terminal;
}
