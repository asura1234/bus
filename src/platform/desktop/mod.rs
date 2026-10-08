//! Host terminal defaults and desktop services.
#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::{
    interactive_shell_command, scrollback_editor_argv, should_draw_host_cursor_by_default,
    should_query_host_terminal_palette,
};
#[cfg(target_os = "linux")]
pub use linux::{open_url, read_clipboard_image, show_desktop_notification, write_clipboard};

#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{
    interactive_shell_command, scrollback_editor_argv, should_draw_host_cursor_by_default,
    should_query_host_terminal_palette,
};
#[cfg(target_os = "macos")]
pub use macos::{open_url, read_clipboard_image, show_desktop_notification, write_clipboard};
