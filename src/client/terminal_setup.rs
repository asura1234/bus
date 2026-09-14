//! Terminal setup and restoration for the rendered client.

use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
#[cfg(unix)]
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
#[cfg(not(windows))]
use crossterm::event::{PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use crossterm::execute;
use crossterm::terminal::{DisableLineWrap, EnableLineWrap};

use super::frame_output::clear_received_kitty_graphics;
use super::terminal_geometry::should_query_host_terminal_theme;

// ---------------------------------------------------------------------------
// Terminal setup / restore
// ---------------------------------------------------------------------------

/// Sets up the terminal for client mode (raw mode, optional mouse, keyboard enhancements).
///
/// Returns a guard that restores the terminal when dropped.
pub(super) fn setup_terminal(mouse_capture: bool) -> io::Result<TerminalGuard> {
    setup_terminal_with_capabilities(true, mouse_capture)
}

/// Sets up a direct attach terminal.
///
/// Direct attach forwards stdin to the attached PTY. When configured, mouse
/// capture lets wheel events drive the attached viewport or reach child
/// programs that requested mouse input.
pub(super) fn setup_direct_attach_terminal(mouse_capture: bool) -> io::Result<TerminalGuard> {
    setup_terminal_with_capabilities(false, mouse_capture)
}

pub(super) fn setup_terminal_with_capabilities(
    enable_client_protocols: bool,
    mouse_capture: bool,
) -> io::Result<TerminalGuard> {
    ratatui::init();
    crate::terminal_modes::clear_host_mouse_reporting(&mut io::stdout())?;
    let host_color_scheme_reports =
        should_enable_host_color_scheme_reports(enable_client_protocols);

    #[cfg(windows)]
    let windows_ssh_session = is_ssh_session();
    #[cfg(windows)]
    let mut windows_virtual_terminal_input =
        if windows_vti_input_backend_enabled() && windows_ssh_session {
            enable_windows_virtual_terminal_input()
        } else {
            WindowsVirtualTerminalInputSetup::default()
        };

    if enable_client_protocols {
        set_mouse_capture(mouse_capture, false)?;
        execute!(io::stdout(), EnableBracketedPaste, EnableFocusChange)?;
        if host_color_scheme_reports {
            write_host_color_scheme_report_mode(&mut io::stdout(), true)?;
        }
        push_keyboard_enhancement_flags()?;
    } else {
        if should_query_host_terminal_theme() {
            write_host_color_scheme_report_mode(&mut io::stdout(), false)?;
        }
        set_mouse_capture(mouse_capture, false)?;
        execute!(io::stdout(), EnableBracketedPaste)?;
    }

    #[cfg(windows)]
    if enable_client_protocols && windows_vti_input_backend_enabled() && !windows_ssh_session {
        windows_virtual_terminal_input = enable_windows_virtual_terminal_input();
    }

    #[cfg(windows)]
    if enable_client_protocols
        && windows_vti_input_backend_enabled()
        && windows_virtual_terminal_input.active
        && windows_win32_input_mode_enabled()
    {
        if let Err(err) = enable_windows_win32_input_mode(&mut io::stdout()) {
            if let Some(mode) = windows_virtual_terminal_input.restore_mode {
                restore_windows_input_mode_value(mode);
            }
            return Err(err);
        }
    }

    let modify_other_keys_mode = enable_client_protocols
        .then(crate::input::host_modify_other_keys_mode)
        .flatten();
    if let Some(mode) = modify_other_keys_mode {
        io::stdout().write_all(mode.set_sequence())?;
        io::stdout().flush()?;
    }

    execute!(io::stdout(), DisableLineWrap)?;

    Ok(TerminalGuard {
        reset_keyboard_enhancements: enable_client_protocols,
        reset_modify_other_keys: modify_other_keys_mode.is_some(),
        reset_host_color_scheme_reports: host_color_scheme_reports,
        restore_claimed: Arc::new(AtomicBool::new(false)),
        restored: false,
        #[cfg(windows)]
        restore_windows_input_mode: windows_virtual_terminal_input.restore_mode,
    })
}

pub(super) fn should_enable_host_color_scheme_reports(enable_client_protocols: bool) -> bool {
    enable_client_protocols && should_query_host_terminal_theme()
}

#[cfg(unix)]
pub(super) fn wait_for_host_palette_query(
    pending: &AtomicBool,
    progress: &AtomicU16,
    idle_timeout: Duration,
    hard_timeout: Duration,
) -> bool {
    let started = Instant::now();
    let hard_deadline = started + hard_timeout;
    let mut idle_deadline = started + idle_timeout;
    let mut observed_progress = progress.load(Ordering::Acquire);
    while pending.load(Ordering::Acquire) {
        let now = Instant::now();
        let current_progress = progress.load(Ordering::Acquire);
        if current_progress != observed_progress {
            observed_progress = current_progress;
            idle_deadline = now + idle_timeout;
        }
        if now >= idle_deadline || now >= hard_deadline {
            return !pending.load(Ordering::Acquire);
        }
        std::thread::sleep(
            (idle_deadline - now)
                .min(hard_deadline - now)
                .min(Duration::from_millis(1)),
        );
    }
    true
}

#[cfg(unix)]
pub(super) fn finish_terminal_input(
    host_palette_query_pending: &AtomicBool,
    host_palette_query_progress: &AtomicU16,
    input_reader_should_quit: &AtomicBool,
    idle_timeout: Duration,
    hard_timeout: Duration,
) -> bool {
    // Keep raw mode active while the reader consumes terminal-generated replies. Restoring echo
    // first would make a slow OSC 4 tail visible at the user's shell prompt.
    let complete = wait_for_host_palette_query(
        host_palette_query_pending,
        host_palette_query_progress,
        idle_timeout,
        hard_timeout,
    );
    input_reader_should_quit.store(true, Ordering::Release);
    complete
}

/// Guard that restores the terminal when dropped.
pub(super) struct TerminalGuard {
    reset_keyboard_enhancements: bool,
    reset_modify_other_keys: bool,
    reset_host_color_scheme_reports: bool,
    restore_claimed: Arc<AtomicBool>,
    restored: bool,
    #[cfg(windows)]
    restore_windows_input_mode: Option<u32>,
}

pub(super) fn write_host_color_scheme_report_mode(
    writer: &mut impl io::Write,
    enabled: bool,
) -> io::Result<()> {
    let sequence = if enabled {
        crate::terminal_theme::HOST_COLOR_SCHEME_REPORT_ENABLE_SEQUENCE
    } else {
        crate::terminal_theme::HOST_COLOR_SCHEME_REPORT_DISABLE_SEQUENCE
    };
    writer.write_all(sequence.as_bytes())?;
    writer.flush()
}

pub(super) fn write_terminal_restore_postlude(
    writer: &mut impl io::Write,
    reset_host_color_scheme_reports: bool,
) -> io::Result<()> {
    if reset_host_color_scheme_reports {
        writer.write_all(
            crate::terminal_theme::HOST_COLOR_SCHEME_REPORT_DISABLE_SEQUENCE.as_bytes(),
        )?;
    }
    // Restore a visible cursor and reset DECSCUSR back to the terminal default.
    writer.write_all(b"\x1b[?25h\x1b[0 q")?;
    writer.flush()
}

pub(super) fn should_draw_host_cursor(mode: crate::config::HostCursorModeConfig) -> bool {
    match mode {
        crate::config::HostCursorModeConfig::Auto => {
            crate::platform::should_draw_host_cursor_by_default()
        }
        crate::config::HostCursorModeConfig::Native => false,
        crate::config::HostCursorModeConfig::Drawn => true,
    }
}

#[cfg(windows)]
#[derive(Default)]
pub(super) struct WindowsVirtualTerminalInputSetup {
    active: bool,
    restore_mode: Option<u32>,
}

#[cfg(windows)]
pub(super) fn enable_windows_virtual_terminal_input() -> WindowsVirtualTerminalInputSetup {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_INPUT,
        STD_INPUT_HANDLE,
    };

    let handle: HANDLE = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        tracing::warn!("failed to get Windows console input handle for VT input");
        return WindowsVirtualTerminalInputSetup::default();
    }

    let mut mode = 0;
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        tracing::warn!("failed to read Windows console input mode for VT input");
        return WindowsVirtualTerminalInputSetup::default();
    }

    let desired = windows_virtual_terminal_input_mode(mode);
    if desired == mode {
        return WindowsVirtualTerminalInputSetup {
            active: true,
            restore_mode: None,
        };
    }

    if unsafe { SetConsoleMode(handle, desired) } == 0 {
        tracing::warn!("failed to enable Windows virtual terminal input");
        return WindowsVirtualTerminalInputSetup::default();
    }

    let mut applied = 0;
    if unsafe { GetConsoleMode(handle, &mut applied) } == 0 {
        tracing::warn!("failed to verify Windows virtual terminal input mode");
        let _ = unsafe { SetConsoleMode(handle, mode) };
        return WindowsVirtualTerminalInputSetup::default();
    }
    if applied & ENABLE_VIRTUAL_TERMINAL_INPUT == 0 {
        tracing::warn!("Windows virtual terminal input bit did not stick");
        let _ = unsafe { SetConsoleMode(handle, mode) };
        return WindowsVirtualTerminalInputSetup::default();
    }

    WindowsVirtualTerminalInputSetup {
        active: true,
        restore_mode: Some(mode),
    }
}

pub(super) fn is_ssh_session() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

#[cfg(windows)]
pub(super) fn windows_vti_input_backend_enabled() -> bool {
    std::env::var("HERDR_WINDOWS_INPUT_BACKEND")
        .map(|backend| !backend.eq_ignore_ascii_case("crossterm"))
        .unwrap_or(true)
}

#[cfg(any(windows, test))]
pub(super) fn windows_virtual_terminal_input_mode(mode: u32) -> u32 {
    mode | 0x0200
}

#[cfg(windows)]
fn restore_windows_input_mode_value(mode: u32) {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{GetStdHandle, SetConsoleMode, STD_INPUT_HANDLE};

    let handle: HANDLE = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return;
    }
    if unsafe { SetConsoleMode(handle, mode) } == 0 {
        tracing::warn!("failed to restore Windows console input mode");
    }
}

pub(super) fn effective_mouse_capture(
    server_enabled: bool,
    direct_attach_preference: bool,
) -> bool {
    server_enabled || direct_attach_preference
}

pub(super) fn effective_sgr_pixel_mouse(
    enabled: bool,
    requested: bool,
    exact_geometry: bool,
) -> bool {
    enabled && requested && exact_geometry
}

pub(super) fn set_mouse_capture(enabled: bool, sgr_pixels: bool) -> io::Result<()> {
    crate::terminal_modes::clear_host_mouse_reporting(&mut io::stdout())?;
    #[cfg(windows)]
    if is_ssh_session() && windows_vti_input_backend_enabled() {
        return crate::terminal_modes::set_windows_ssh_mouse_reporting(
            &mut io::stdout(),
            enabled,
            sgr_pixels,
        );
    }
    if enabled {
        execute!(io::stdout(), EnableMouseCapture)?;
        if sgr_pixels {
            io::stdout().write_all(b"\x1b[?1016h")?;
            io::stdout().flush()?;
        }
        Ok(())
    } else {
        match execute!(io::stdout(), DisableMouseCapture) {
            Ok(()) => Ok(()),
            #[cfg(windows)]
            Err(err) if err.to_string() == "Initial console modes not set" => Ok(()),
            Err(err) => Err(err),
        }
    }
}

fn restore_terminal_state_once(
    restore_claimed: &AtomicBool,
    reset_keyboard_enhancements: bool,
    reset_modify_other_keys: bool,
    reset_host_color_scheme_reports: bool,
    #[cfg(windows)] restore_windows_input_mode: Option<u32>,
) -> io::Result<()> {
    if restore_claimed.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    restore_terminal_state(
        reset_keyboard_enhancements,
        reset_modify_other_keys,
        reset_host_color_scheme_reports,
        #[cfg(windows)]
        restore_windows_input_mode,
    )
}

fn restore_terminal_state(
    reset_keyboard_enhancements: bool,
    reset_modify_other_keys: bool,
    reset_host_color_scheme_reports: bool,
    #[cfg(windows)] restore_windows_input_mode: Option<u32>,
) -> io::Result<()> {
    let _ = clear_received_kitty_graphics(&mut io::stdout());

    // Reset modifyOtherKeys if we enabled it.
    if reset_modify_other_keys {
        let _ = io::stdout().write_all(b"\x1b[>4;0m");
        let _ = io::stdout().flush();
    }

    if reset_keyboard_enhancements {
        let _ = pop_keyboard_enhancement_flags();
    }

    let _ = execute!(
        io::stdout(),
        EnableLineWrap,
        DisableFocusChange,
        DisableBracketedPaste
    );
    let _ = set_mouse_capture(false, false);
    #[cfg(windows)]
    if let Some(mode) = restore_windows_input_mode {
        restore_windows_input_mode_value(mode);
    }

    let restore_result = ratatui::try_restore();
    let postlude_result =
        write_terminal_restore_postlude(&mut io::stdout(), reset_host_color_scheme_reports);

    #[cfg(windows)]
    if windows_vti_input_backend_enabled() && windows_win32_input_mode_enabled() {
        let _ = disable_windows_win32_input_mode(&mut io::stdout());
    }

    restore_result.and(postlude_result)
}

#[cfg(not(windows))]
fn push_keyboard_enhancement_flags() -> io::Result<()> {
    execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(crate::input::ime_compatible_keyboard_enhancement_flags())
    )
}

#[cfg(windows)]
fn push_keyboard_enhancement_flags() -> io::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
fn pop_keyboard_enhancement_flags() -> io::Result<()> {
    execute!(io::stdout(), PopKeyboardEnhancementFlags)
}

#[cfg(windows)]
fn pop_keyboard_enhancement_flags() -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn windows_win32_input_mode_enabled() -> bool {
    std::env::var("HERDR_WINDOWS_INPUT_PROBE")
        .map(|probe| probe.eq_ignore_ascii_case("win32"))
        .unwrap_or(true)
}

#[cfg(windows)]
fn enable_windows_win32_input_mode(writer: &mut impl std::io::Write) -> io::Result<()> {
    writer.write_all(b"\x1b[?9001h")?;
    writer.flush()
}

#[cfg(windows)]
fn disable_windows_win32_input_mode(writer: &mut impl std::io::Write) -> io::Result<()> {
    writer.write_all(b"\x1b[?9001l")?;
    writer.flush()
}

impl TerminalGuard {
    /// Captures the restoration state for use by the process panic hook.
    pub(super) fn panic_restore(&self) -> impl Fn() + Send + Sync + 'static {
        let restore_claimed = self.restore_claimed.clone();
        let reset_keyboard_enhancements = self.reset_keyboard_enhancements;
        let reset_modify_other_keys = self.reset_modify_other_keys;
        let reset_host_color_scheme_reports = self.reset_host_color_scheme_reports;
        #[cfg(windows)]
        let restore_windows_input_mode = self.restore_windows_input_mode;
        move || {
            let _ = restore_terminal_state_once(
                &restore_claimed,
                reset_keyboard_enhancements,
                reset_modify_other_keys,
                reset_host_color_scheme_reports,
                #[cfg(windows)]
                restore_windows_input_mode,
            );
        }
    }

    pub(super) fn restore(mut self) -> io::Result<()> {
        self.restored = true;
        restore_terminal_state_once(
            &self.restore_claimed,
            self.reset_keyboard_enhancements,
            self.reset_modify_other_keys,
            self.reset_host_color_scheme_reports,
            #[cfg(windows)]
            self.restore_windows_input_mode,
        )
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore_terminal_state_once(
                &self.restore_claimed,
                self.reset_keyboard_enhancements,
                self.reset_modify_other_keys,
                self.reset_host_color_scheme_reports,
                #[cfg(windows)]
                self.restore_windows_input_mode,
            );
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU16;
    use std::time::Duration;

    #[test]
    fn shutdown_drain_stays_open_while_palette_replies_advance() {
        let pending = Arc::new(AtomicBool::new(true));
        let progress = Arc::new(AtomicU16::new(0));
        let advancing_pending = pending.clone();
        let advancing_progress = progress.clone();
        let replies = std::thread::spawn(move || {
            for reply_count in 1..=4 {
                std::thread::sleep(Duration::from_millis(40));
                advancing_progress.store(reply_count, Ordering::Release);
            }
            advancing_pending.store(false, Ordering::Release);
        });

        assert!(wait_for_host_palette_query(
            &pending,
            &progress,
            Duration::from_millis(100),
            Duration::from_millis(500),
        ));
        replies.join().unwrap();
    }

    #[test]
    fn shutdown_drain_stops_when_palette_replies_stall() {
        let pending = AtomicBool::new(true);
        let progress = AtomicU16::new(0);

        assert!(!wait_for_host_palette_query(
            &pending,
            &progress,
            Duration::from_millis(10),
            Duration::from_millis(100),
        ));
    }

    #[test]
    fn shutdown_waits_for_final_palette_reply_before_restoring_echo() {
        let pending = Arc::new(AtomicBool::new(true));
        let progress = AtomicU16::new(0);
        let completed = pending.clone();
        let reply = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            completed.store(false, Ordering::Release);
        });

        assert!(wait_for_host_palette_query(
            &pending,
            &progress,
            Duration::from_millis(100),
            Duration::from_millis(100),
        ));
        reply.join().unwrap();
    }

    #[test]
    fn shutdown_keeps_input_reader_alive_until_palette_drain_finishes() {
        let pending = Arc::new(AtomicBool::new(true));
        let progress = AtomicU16::new(0);
        let reader_quit = Arc::new(AtomicBool::new(false));
        let completed = pending.clone();
        let observed_quit = reader_quit.clone();
        let reply = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            assert!(!observed_quit.load(Ordering::Acquire));
            completed.store(false, Ordering::Release);
        });

        assert!(finish_terminal_input(
            &pending,
            &progress,
            &reader_quit,
            Duration::from_millis(100),
            Duration::from_millis(100),
        ));
        assert!(reader_quit.load(Ordering::Acquire));
        reply.join().unwrap();
    }
}
