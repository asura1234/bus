//! Windows platform facade and console primitives.
mod daemon;
mod desktop;
mod fs;
mod process;
mod shell;
pub(crate) use daemon::configure_background_command_platform;
#[cfg(test)]
use daemon::current_job_kills_processes_on_close;
pub use daemon::current_process_is_detached_server_daemon;
pub use daemon::detach_server_daemon_command;
pub use daemon::launch_server_daemon_command;
#[cfg(test)]
use daemon::launch_server_daemon_with_wmi;
#[cfg(test)]
use daemon::windows_environment_key_cmp;
#[cfg(test)]
use desktop::copy_wide_truncated;
pub use desktop::open_url;
pub use desktop::read_clipboard_image;
pub use desktop::show_desktop_notification;
pub use desktop::write_clipboard;
pub(crate) use fs::create_remote_private_dir;
pub(crate) use fs::create_remote_ssh_config_file;
pub(crate) use fs::replace_file;
pub(crate) use process::available_pane_shell;
#[cfg(test)]
use process::available_pane_shell_from_snapshot;
#[cfg(test)]
use process::foreground::descendant_entries;
#[cfg(test)]
use process::foreground::foreground_job_from_entry;
#[cfg(test)]
use process::foreground::CachedForegroundSelection;
#[cfg(test)]
use process::foreground::ForegroundSelectionCache;
#[cfg(test)]
use process::foreground::FOREGROUND_SELECTION_CACHE_RETENTION;
#[cfg(test)]
use process::foreground::FOREGROUND_SELECTION_RECHECK;
pub use process::foreground_group_leader_job;
pub(crate) use process::peb::apply_pane_runtime_marker_platform;
#[cfg(test)]
use process::peb::environment_variable_from_utf16;
#[cfg(test)]
use process::peb::next_pane_runtime_marker;
#[cfg(test)]
use process::peb::process_runtime_marker;
#[cfg(test)]
use process::peb::PANE_RUNTIME_MARKER_ENV_VAR;
pub(crate) use process::process_birth;
pub use process::process_cwd;
pub use process::process_exists;
pub(crate) use process::process_parent;
pub use process::session_processes;
#[cfg(test)]
use process::session_processes_from_snapshot;
pub use process::signal_processes;
#[cfg(test)]
use process::snapshot::ProcessIdentity;
pub(crate) use process::snapshot::ProcessSnapshot;
#[cfg(test)]
use process::snapshot::ProcessSnapshotCache;
#[cfg(test)]
pub(crate) use process::snapshot::WindowsProcessCommand;
pub(crate) use process::snapshot::WindowsProcessEntry;
pub(crate) use shell::interactive_shell_command;
pub(crate) use shell::remote_reattach_program;
pub(crate) use shell::scrollback_editor_argv;
#[cfg(test)]
use shell::scrollback_editor_argv_with_env;
use std::ptr::null_mut;
#[cfg(test)]
use std::sync::OnceLock;
use std::time::Duration;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::ToUnicodeEx;
use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

mod clipboard_image;

pub(crate) fn classify_child_exit(status: &portable_pty::ExitStatus) -> super::ChildExitReason {
    // STATUS_CONTROL_C_EXIT is reported without a Unix signal by portable-pty.
    if status.exit_code() == 0xC000013A {
        super::ChildExitReason::Interrupted
    } else {
        super::ChildExitReason::Exited
    }
}

pub(crate) fn wait_client_stream_readable(
    _stream: &crate::platform::ipc::LocalStream,
) -> std::io::Result<()> {
    // Sync named pipes have no read timeout. The caller peeks before each read and checks its
    // cancellation flag between polls, including when a frame arrives in several fragments.
    std::thread::sleep(Duration::from_millis(2));
    Ok(())
}

pub(super) fn read_terminal_grid_size() -> std::io::Result<(u16, u16)> {
    crossterm::terminal::size()
}

pub(crate) fn terminal_title_for_presentation(title: &str) -> &str {
    title.strip_prefix("Administrator: ").unwrap_or(title)
}

pub(crate) fn prepare_paste_text_for_pty_platform(text: String) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Resolves against the current foreground layout because asynchronous console
/// records do not retain the layout that was active when the key was pressed.
pub(crate) fn resolve_base_printable_key(vk: u16, scan: u16) -> Option<char> {
    // SAFETY: Win32 owns the handles; the fixed buffers match the API lengths.
    unsafe {
        let thread_id = GetWindowThreadProcessId(GetForegroundWindow(), null_mut());
        let layout = GetKeyboardLayout(thread_id);

        let key_state = [0u8; 256];
        let mut output = [0u16; 2];
        let written = ToUnicodeEx(
            vk.into(),
            scan.into(),
            key_state.as_ptr(),
            output.as_mut_ptr(),
            output.len() as i32,
            0x4,
            layout,
        );
        let units = output.get(..usize::try_from(written).ok()?)?;
        let mut chars = char::decode_utf16(units.iter().copied());
        let ch = chars.next()?.ok()?;
        (chars.next().is_none() && !ch.is_control()).then_some(ch)
    }
}

pub(crate) fn should_draw_host_cursor_by_default() -> bool {
    true
}

pub(crate) fn should_query_host_terminal_palette() -> bool {
    false
}

/// The machine's node name, as shown by tmux's `#h`.
pub(crate) fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
pub(crate) fn local_datetime() -> Option<time::PrimitiveDateTime> {
    let mut timestamp: libc::time_t = 0;
    if unsafe { libc::time(&mut timestamp) } == -1 {
        return None;
    }
    local_datetime_at(timestamp)
}

pub(crate) fn local_datetime_at(seconds: i64) -> Option<time::PrimitiveDateTime> {
    let timestamp = libc::time_t::try_from(seconds).ok()?;
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_s(&mut local, &timestamp) } != 0 {
        return None;
    }
    let month = time::Month::try_from(u8::try_from(local.tm_mon + 1).ok()?).ok()?;
    let date = time::Date::from_calendar_date(
        local.tm_year + 1900,
        month,
        u8::try_from(local.tm_mday).ok()?,
    )
    .ok()?;
    let time = time::Time::from_hms(
        u8::try_from(local.tm_hour).ok()?,
        u8::try_from(local.tm_min).ok()?,
        u8::try_from(local.tm_sec).ok()?,
    )
    .ok()?;
    Some(time::PrimitiveDateTime::new(date, time))
}

pub fn raise_server_nofile_limit() {}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
#[path = "tests/shell_test.rs"]
mod tests;

pub(crate) use process::foreground::pane_foreground_job_with;
