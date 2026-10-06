use super::*;
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn resize_signal_reports_even_when_polled_size_is_unchanged() {
    let size = (120, 40, 8, 16, true);
    assert!(resize_report_required(true, size, size));
    assert!(!resize_report_required(false, size, size));
    assert!(resize_report_required(false, (120, 41, 8, 16, true), size));
    assert!(resize_report_required(false, (120, 40, 9, 18, true), size));
    assert!(resize_report_required(false, (120, 40, 8, 16, false), size));
}

#[test]
fn unavailable_terminal_grid_is_not_fabricated() {
    let reported_cell_size = AtomicU64::new(0);
    let err = current_terminal_geometry_with(false, false, &reported_cell_size, None, None, || {
        Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "terminal is gone",
        ))
    })
    .expect_err("an unavailable terminal must not produce fallback geometry");

    assert_eq!(err.kind(), io::ErrorKind::NotConnected);
}

#[test]
fn missing_pixel_geometry_keeps_a_valid_terminal_grid() {
    let reported_cell_size = AtomicU64::new(0);
    let geometry = current_terminal_geometry_with(
        true,
        true,
        &reported_cell_size,
        Some((9, 18)),
        None,
        || Ok((80, 24)),
    )
    .expect("grid geometry remains valid without pixel dimensions");

    assert_eq!(geometry, (80, 24, 9, 18, false));
}

#[test]
fn direct_graphics_profile_is_narrow_and_transport_safe() {
    for (program, term, kitty, expected) in [
        ("ghostty", "", false, true),
        ("WezTerm", "", false, true),
        ("", "xterm-kitty", false, true),
        ("", "xterm-256color", true, true),
        ("", "xterm-256color", false, false),
    ] {
        assert_eq!(
            direct_graphics_profile_values(program, term, kitty, false, true),
            expected
        );
    }
    assert!(!direct_graphics_profile_values(
        "ghostty", "", false, true, true
    ));
    assert!(!direct_graphics_profile_values(
        "ghostty", "", false, false, false
    ));
}

fn restore_env_var(key: &str, value: Option<OsString>) {
    if let Some(value) = value {
        std::env::set_var(key, value);
    } else {
        std::env::remove_var(key);
    }
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        restore_env_var(self.key, self.previous.clone());
    }
}

#[test]
fn windows_virtual_terminal_input_mode_sets_only_vti_bit() {
    assert_eq!(windows_virtual_terminal_input_mode(0x01f0), 0x03f0);
    assert_eq!(windows_virtual_terminal_input_mode(0x03f0), 0x03f0);
}

struct EnvVarsRemovedGuard {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl EnvVarsRemovedGuard {
    fn new(keys: &[&'static str]) -> Self {
        let previous: Vec<_> = keys
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        for key in keys {
            std::env::remove_var(key);
        }
        Self { previous }
    }
}

impl Drop for EnvVarsRemovedGuard {
    fn drop(&mut self) {
        for (key, value) in self.previous.clone() {
            restore_env_var(key, value);
        }
    }
}

#[test]
fn host_cursor_policy_auto_uses_platform_default() {
    assert_eq!(
        should_draw_host_cursor(crate::config::HostCursorModeConfig::Auto),
        crate::platform::should_draw_host_cursor_by_default()
    );
}

#[test]
fn host_cursor_policy_native_and_drawn_override_auto_detection() {
    let _guard = env_lock().lock().unwrap();
    let _env = EnvVarGuard::set("TERM_PROGRAM", "WezTerm");

    assert!(!should_draw_host_cursor(
        crate::config::HostCursorModeConfig::Native
    ));
    assert!(should_draw_host_cursor(
        crate::config::HostCursorModeConfig::Drawn
    ));
}

#[test]
fn graphics_bytes_are_written_inside_synchronized_blit_with_saved_cursor() {
    let mut output = Vec::new();
    write_encoded_frame_with_graphics(
        &mut output,
        b"\x1b[?2026htext\x1b[?2026lcursor",
        b"graphics",
    )
    .unwrap();

    assert_eq!(
        output,
        b"\x1b[?2026htext\x1b7graphics\x1b8\x1b[?2026lcursor"
    );
}

#[test]
fn empty_graphics_writes_only_blit_frame() {
    let mut output = Vec::new();
    write_encoded_frame_with_graphics(&mut output, b"text", b"").unwrap();

    assert_eq!(output, b"text");
}

#[test]
fn terminal_frame_kitty_detection_matches_apc_prefix() {
    assert!(contains_kitty_graphics_bytes(b"text\x1b_Ga=p;\x1b\\"));
    assert!(!contains_kitty_graphics_bytes(b"text\x1b[?2026h"));
}

#[test]
fn kitty_graphics_image_id_parser_tracks_herdr_ids_only() {
    let ids = kitty_graphics_image_ids(
        b"text\x1b_Ga=t,t=d,f=32,s=1,v=1,i=10023,q=2;AAAA\x1b\\\x1b_Ga=p,i=10023,p=7;\x1b\\",
    );
    assert_eq!(ids, vec![10023, 10023]);
}

#[test]
fn kitty_graphics_cleanup_deletes_tracked_images_not_all_images() {
    record_received_kitty_graphics(b"\x1b_Ga=t,i=123,q=2;AAAA\x1b\\");
    let mut output = Vec::new();
    clear_received_kitty_graphics(&mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("a=d,d=I,i=123"));
    assert!(!text.contains("d=A"));
}

#[test]
fn write_host_terminal_appearance_query_emits_mode_2031_query() {
    let mut output = Vec::new();
    write_host_terminal_appearance_query(&mut output).unwrap();
    assert_eq!(output, b"\x1b[?996n");
}

#[test]
fn write_host_terminal_theme_query_emits_osc_queries() {
    let mut output = Vec::new();
    let palette_query_pending = AtomicBool::new(false);
    let palette_query_progress = AtomicU16::new(42);
    write_host_terminal_theme_query(&mut output, &palette_query_pending, &palette_query_progress)
        .unwrap();
    assert_eq!(
        palette_query_pending.load(Ordering::Acquire),
        crate::platform::should_query_host_terminal_palette()
    );
    assert_eq!(palette_query_progress.load(Ordering::Acquire), 0);
    assert_eq!(
        output,
        crate::terminal_theme::host_terminal_theme_query_sequence(
            crate::platform::should_query_host_terminal_palette(),
        )
        .as_bytes()
    );
    assert!(
        !output
            .windows(crate::terminal_theme::HOST_COLOR_SCHEME_QUERY_SEQUENCE.len())
            .any(|window| window
                == crate::terminal_theme::HOST_COLOR_SCHEME_QUERY_SEQUENCE.as_bytes())
    );
}

#[test]
fn write_host_color_scheme_report_mode_emits_mode_sequences() {
    let mut output = Vec::new();
    write_host_color_scheme_report_mode(&mut output, true).unwrap();
    write_host_color_scheme_report_mode(&mut output, false).unwrap();

    let mut expected = Vec::new();
    expected.extend_from_slice(
        crate::terminal_theme::HOST_COLOR_SCHEME_REPORT_ENABLE_SEQUENCE.as_bytes(),
    );
    expected.extend_from_slice(
        crate::terminal_theme::HOST_COLOR_SCHEME_REPORT_DISABLE_SEQUENCE.as_bytes(),
    );
    assert_eq!(output, expected);
}

#[test]
fn color_scheme_change_event_requests_host_theme_query() {
    let events = crate::raw_input::parse_raw_input_bytes_sync(b"\x1b[?997;1n");

    assert!(crate::raw_input::events_require_host_terminal_theme_query(
        &events
    ));
}

#[test]
fn host_terminal_theme_query_is_disabled_on_windows() {
    assert_eq!(should_query_host_terminal_theme(), !cfg!(windows));
}

#[test]
fn write_host_cell_size_query_emits_xtwinops_request() {
    let mut output = Vec::new();
    write_host_cell_size_query(&mut output).unwrap();

    assert_eq!(output, b"\x1b[16t");
}

#[test]
fn host_cell_size_query_is_disabled_on_windows() {
    assert_eq!(should_query_host_cell_size(), !cfg!(windows));
}

#[test]
fn cell_size_fallback_prefers_reported_then_previous_size() {
    assert_eq!(cell_size_fallback(0, None), (8, 16));
    assert_eq!(cell_size_fallback(0, Some((11, 22))), (11, 22));
    assert_eq!(
        cell_size_fallback(pack_cell_size(10, 21), Some((11, 22))),
        (10, 21)
    );
    assert_eq!(cell_size_fallback(pack_cell_size(10, 0), None), (8, 16));
    assert_eq!(cell_size_fallback(pack_cell_size(0, 21), None), (8, 16));
}

#[test]
fn reported_cell_size_is_taken_from_host_cell_size_events() {
    let events = crate::raw_input::parse_raw_input_bytes_sync(b"\x1b[?997;1n");
    assert_eq!(
        super::terminal_geometry::reported_cell_size_from_events(&events),
        None
    );

    let events = crate::raw_input::parse_raw_input_bytes_sync(b"\x1b[6;21;10t\x1b[6;18;9t");
    assert_eq!(
        super::terminal_geometry::reported_cell_size_from_events(&events),
        Some((9, 18))
    );
}

#[test]
fn color_scheme_reports_are_enabled_only_for_full_clients() {
    assert_eq!(
        should_enable_host_color_scheme_reports(true),
        !cfg!(windows)
    );
    assert!(!should_enable_host_color_scheme_reports(false));
}

#[test]
fn terminal_restore_postlude_restores_visible_default_cursor() {
    let mut output = Vec::new();
    write_terminal_restore_postlude(&mut output, false).unwrap();
    assert_eq!(output, b"\x1b[?25h\x1b[0 q");
}

#[test]
fn direct_attach_mouse_capture_combines_local_preference_with_child_demand() {
    assert!(effective_mouse_capture(false, true));
    assert!(effective_mouse_capture(true, false));
    assert!(!effective_mouse_capture(false, false));
    assert!(effective_sgr_pixel_mouse(true, true, true));
    assert!(!effective_sgr_pixel_mouse(true, true, false));
}

#[test]
fn terminal_restore_postlude_disables_color_scheme_reports_when_enabled() {
    let mut output = Vec::new();
    write_terminal_restore_postlude(&mut output, true).unwrap();

    let mut expected = Vec::new();
    expected.extend_from_slice(
        crate::terminal_theme::HOST_COLOR_SCHEME_REPORT_DISABLE_SEQUENCE.as_bytes(),
    );
    expected.extend_from_slice(b"\x1b[?25h\x1b[0 q");
    assert_eq!(output, expected);
}

#[test]
fn client_error_display_connection_failed() {
    let err = ClientError::ConnectionFailed(io::Error::new(
        io::ErrorKind::ConnectionRefused,
        "connection refused",
    ));
    let msg = err.to_string();
    assert!(
        msg.contains("failed to connect to server"),
        "should mention connection failure: {msg}"
    );
    assert!(
        msg.contains("herdr server"),
        "should suggest starting server: {msg}"
    );
}

#[test]
fn client_error_display_handshake_rejected() {
    let err = ClientError::HandshakeRejected {
        version: 1,
        error: "incompatible".into(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("rejected handshake"),
        "should mention rejection: {msg}"
    );
    assert!(msg.contains("incompatible"), "should include error: {msg}");
}

#[test]
fn client_error_display_server_shutdown() {
    let err = ClientError::ServerShutdown {
        reason: Some("maintenance".into()),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("server shut down"),
        "should mention shutdown: {msg}"
    );
    assert!(msg.contains("maintenance"), "should include reason: {msg}");
}

#[test]
fn client_error_display_server_shutdown_no_reason() {
    let err = ClientError::ServerShutdown { reason: None };
    let msg = err.to_string();
    assert!(
        msg.contains("server shut down"),
        "should mention shutdown: {msg}"
    );
}

#[test]
fn client_error_display_detached_suggests_the_bus_reattach_command() {
    let _guard = env_lock().lock().unwrap();
    let err = ClientError::ServerShutdown {
        reason: Some("detached".into()),
    };
    // The `bus resume ID` form is covered in session tests without touching
    // BUS_SESSION_ID, which Bus settings tests read concurrently.
    let _env = EnvVarsRemovedGuard::new(&["BUS_SESSION_ID"]);
    let msg = err.to_string();
    assert!(msg.contains("Run `bus` to reattach"), "{msg}");
}

#[test]
fn client_error_display_connection_lost() {
    let _guard = env_lock().lock().unwrap();
    let err = ClientError::ConnectionLost(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"));
    let msg = err.to_string();
    assert!(
        msg.contains("lost connection to server"),
        "should mention lost connection: {msg}"
    );
}

#[test]
fn sound_from_notify_message_maps_done() {
    assert_eq!(
        sound_from_notify_message("agent done"),
        Some(crate::sound::Sound::Done)
    );
}

#[test]
fn sound_from_notify_message_maps_attention() {
    assert_eq!(
        sound_from_notify_message("agent attention"),
        Some(crate::sound::Sound::Request)
    );
}

#[test]
fn sound_from_notify_message_rejects_unknown_payloads() {
    assert_eq!(sound_from_notify_message("toast"), None);
}

#[test]
fn reload_local_client_config_refreshes_local_client_presentation_state() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let path = std::env::temp_dir().join(format!(
        "herdr-client-config-reload-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &path,
        "[ui]\nredraw_on_focus_gained = false\nhost_cursor = \"drawn\"\nmouse_capture = false\n",
    )
    .unwrap();
    let path_string = path.to_string_lossy().to_string();
    let _env = EnvVarGuard::set(crate::config::CONFIG_PATH_ENV_VAR, &path_string);
    let mut sound_config = crate::config::SoundConfig::default();
    let mut redraw_on_focus_gained = true;
    let mut draw_host_cursor = false;
    let mut mouse_capture = true;

    reload_local_client_config(
        &mut sound_config,
        &mut redraw_on_focus_gained,
        &mut draw_host_cursor,
        &mut mouse_capture,
    );

    assert!(!redraw_on_focus_gained);
    assert!(draw_host_cursor);
    assert!(!mouse_capture);
    let _ = std::fs::remove_file(path);
}

#[test]
fn reload_local_client_config_keeps_ui_preferences_when_ui_is_invalid() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let path = std::env::temp_dir().join(format!(
        "herdr-client-invalid-ui-reload-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, "[ui]\nmouse_capture = \"invalid\"\n").unwrap();
    let path_string = path.to_string_lossy().to_string();
    let _env = EnvVarGuard::set(crate::config::CONFIG_PATH_ENV_VAR, &path_string);
    let mut sound_config = crate::config::SoundConfig::default();
    let mut redraw_on_focus_gained = false;
    let mut draw_host_cursor = true;
    let mut mouse_capture = false;

    reload_local_client_config(
        &mut sound_config,
        &mut redraw_on_focus_gained,
        &mut draw_host_cursor,
        &mut mouse_capture,
    );

    assert!(!mouse_capture);
    assert!(!redraw_on_focus_gained);
    assert!(draw_host_cursor);
    let _ = std::fs::remove_file(path);
}

#[test]
fn toast_notify_from_server_is_emitted_even_when_attach_config_was_off() {
    let sound_config = crate::config::SoundConfig::default();
    let mut emitted = None;

    handle_notify_with_notifiers(
        NotifyKind::Toast,
        "pi finished",
        Some("workspace 1"),
        &sound_config,
        |title, body| {
            emitted = Some((title.to_string(), body.map(str::to_string)));
            Ok(true)
        },
        |_, _| Ok(false),
    );

    assert_eq!(
        emitted,
        Some(("pi finished".to_string(), Some("workspace 1".to_string())))
    );
}

#[test]
fn system_toast_notify_from_server_uses_system_notifier() {
    let sound_config = crate::config::SoundConfig::default();
    let mut emitted = None;

    handle_notify_with_notifiers(
        NotifyKind::SystemToast,
        "pi finished",
        Some("workspace 1"),
        &sound_config,
        |_, _| Ok(false),
        |title, body| {
            emitted = Some((title.to_string(), body.map(str::to_string)));
            Ok(true)
        },
    );

    assert_eq!(
        emitted,
        Some(("pi finished".to_string(), Some("workspace 1".to_string())))
    );
}

#[test]
fn system_toast_notify_preserves_colon_in_title() {
    let sound_config = crate::config::SoundConfig::default();
    let mut emitted = None;

    handle_notify_with_notifiers(
        NotifyKind::SystemToast,
        "build: failed",
        Some("api workspace"),
        &sound_config,
        |_, _| Ok(false),
        |title, body| {
            emitted = Some((title.to_string(), body.map(str::to_string)));
            Ok(true)
        },
    );

    assert_eq!(
        emitted,
        Some((
            "build: failed".to_string(),
            Some("api workspace".to_string())
        ))
    );
}

#[test]
fn decode_clipboard_payload_decodes_base64() {
    assert_eq!(decode_clipboard_payload("dGVzdA=="), Some(b"test".to_vec()));
}

#[test]
fn ioctl_cell_size_accepts_fractional_terminal_geometry() {
    assert_eq!(ioctl_cell_size(80, 24, 800, 480), Some((10, 20)));
    assert_eq!(ioctl_cell_size(80, 24, 805, 480), Some((10, 20)));
    assert_eq!(ioctl_cell_size(80, 24, 800, 485), Some((10, 20)));
    assert_eq!(ioctl_cell_size(80, 24, 0, 485), None);
}

#[test]
fn decode_clipboard_payload_rejects_invalid_base64() {
    assert_eq!(decode_clipboard_payload("not-base64!!!"), None);
}

#[test]
fn forward_clipboard_uses_local_clipboard_path() {
    unsafe {
        std::env::set_var("SSH_CONNECTION", "1 2 3 4");
    }
    assert!(forward_clipboard("dGVzdA=="));
    assert!(!forward_clipboard("not base64"));
    unsafe {
        std::env::remove_var("SSH_CONNECTION");
    }
}
