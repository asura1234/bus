use super::ffi;

pub const MOD_SHIFT: u16 = ffi::GHOSTTY_MODS_SHIFT as u16;
pub const MOD_CTRL: u16 = ffi::GHOSTTY_MODS_CTRL as u16;
pub const MOD_ALT: u16 = ffi::GHOSTTY_MODS_ALT as u16;
pub const MOD_SUPER: u16 = ffi::GHOSTTY_MODS_SUPER as u16;

pub const MOUSE_ACTION_PRESS: ffi::GhosttyMouseAction =
    ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS;
pub const MOUSE_ACTION_RELEASE: ffi::GhosttyMouseAction =
    ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_RELEASE;
pub const MOUSE_ACTION_MOTION: ffi::GhosttyMouseAction =
    ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_MOTION;
pub const MOUSE_BUTTON_LEFT: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_LEFT;
pub const MOUSE_BUTTON_RIGHT: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_RIGHT;
pub const MOUSE_BUTTON_MIDDLE: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_MIDDLE;
pub const MOUSE_BUTTON_WHEEL_UP: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FOUR;
pub const MOUSE_BUTTON_WHEEL_DOWN: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FIVE;
pub const MOUSE_BUTTON_WHEEL_LEFT: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_SIX;
pub const MOUSE_BUTTON_WHEEL_RIGHT: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_SEVEN;
pub const MOUSE_FORMAT_SGR: ffi::GhosttyMouseFormat =
    ffi::GhosttyMouseFormat_GHOSTTY_MOUSE_FORMAT_SGR;
pub const MOUSE_FORMAT_SGR_PIXELS: ffi::GhosttyMouseFormat =
    ffi::GhosttyMouseFormat_GHOSTTY_MOUSE_FORMAT_SGR_PIXELS;

pub const MODE_APPLICATION_CURSOR_KEYS: u16 = 1;
pub const MODE_FOCUS_EVENT: u16 = 1004;
pub const MODE_MOUSE_ALTERNATE_SCROLL: u16 = 1007;
pub const MODE_MOUSE_SGR_PIXELS: u16 = 1016;
pub const MODE_BRACKETED_PASTE: u16 = 2004;
pub const MODE_SYNCHRONIZED_OUTPUT: u16 = 2026;
#[cfg(test)]
pub const MODE_GRAPHEME_CLUSTER: u16 = 2027;
pub const MODE_COLOR_SCHEME_REPORT: u16 = 2031;
// These are documented in vendor/libghostty-vt/include/ghostty/vt/terminal.h,
// but the generated bindings do not currently expose named constants for them.
pub(super) const TERMINAL_DATA_COLOR_FOREGROUND: ffi::GhosttyTerminalData = 18;
pub(super) const TERMINAL_DATA_COLOR_BACKGROUND: ffi::GhosttyTerminalData = 19;
pub(super) const TERMINAL_DATA_COLOR_CURSOR: ffi::GhosttyTerminalData = 20;

pub(super) const KITTY_IMAGE_STORAGE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const APC_MAX_BYTES: usize = 16 * 1024 * 1024;
pub(super) const APC_MAX_BYTES_KITTY: usize = 16 * 1024 * 1024;
pub(crate) const KITTY_UNICODE_PLACEHOLDER: u32 = 0x10EEEE;
// The vendored C headers expose these placement fields, but the checked-in
// generated bindings predate the names. Keep the explicit values aligned with
// vendor/libghostty-vt/include/ghostty/vt/kitty_graphics.h.
pub(super) const KITTY_PLACEMENT_DATA_IS_VIRTUAL: ffi::GhosttyKittyGraphicsPlacementData = 3;
pub(super) const KITTY_PLACEMENT_DATA_COLUMNS: ffi::GhosttyKittyGraphicsPlacementData = 10;
pub(super) const KITTY_PLACEMENT_DATA_ROWS: ffi::GhosttyKittyGraphicsPlacementData = 11;
