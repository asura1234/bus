use super::*;

fn supplied_foreground_job(pid: u32, name: &str) -> crate::platform::ForegroundJob {
    crate::platform::ForegroundJob {
        process_group_id: pid,
        processes: vec![crate::platform::ForegroundProcess {
            pid,
            name: name.to_string(),
            argv0: Some(name.to_string()),
            argv: Some(vec![name.to_string()]),
            cmdline: Some(name.to_string()),
        }],
    }
}

#[test]
fn foreground_fact_probes_stay_lazy_for_output_and_theme_restore() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 3, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    let no_probe = |_| -> Option<crate::platform::ForegroundJob> { panic!("unexpected probe") };
    pane.process_pty_bytes(pane_id, 7, b"ordinary output", &tx, no_probe);
    pane.process_pty_bytes(
        pane_id,
        0,
        b"\x1b]11;rgb:11/22/33\x07\x1b[3J",
        &tx,
        no_probe,
    );
    assert!(!pane.maybe_restore_host_terminal_theme(pane_id, 7, || panic!("no owner")));
    pane.process_pty_bytes(pane_id, 7, b"\x1b[?1049h", &tx, no_probe);
    pane.process_pty_bytes(pane_id, 7, b"\x1b[3J", &tx, no_probe);
}

#[test]
fn supplied_foreground_jobs_preserve_droid_scrollback_filtering() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 3, 100).unwrap();
    for _ in 0..10 {
        terminal.write(b"history\r\n");
    }
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    let before = pane.scroll_metrics().unwrap().max_offset_from_bottom;
    assert!(before > 0);
    let probes = std::cell::Cell::new(0);
    pane.process_pty_bytes(pane_id, 7, b"\x1b[3J\x1b[?3J", &tx, |pid| {
        assert_eq!(pid, 7);
        probes.set(probes.get() + 1);
        Some(supplied_foreground_job(42, "droid"))
    });
    assert_eq!(probes.get(), 1);
    assert_eq!(
        pane.scroll_metrics().unwrap().max_offset_from_bottom,
        before
    );
    pane.process_pty_bytes(pane_id, 7, b"\x1b[3J", &tx, |_| {
        Some(supplied_foreground_job(7, "shell"))
    });
    assert_eq!(pane.scroll_metrics().unwrap().max_offset_from_bottom, 0);
}

#[test]
fn supplied_foreground_jobs_keep_transient_color_ownership_and_restore() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 3, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::utils::theme::color::TerminalTheme {
        background: Some(crate::utils::theme::color::RgbColor { r: 1, g: 2, b: 3 }),
        ..Default::default()
    });
    let probes = std::cell::Cell::new(0);
    let probe = |pid| {
        assert_eq!(pid, 7);
        probes.set(probes.get() + 1);
        Some(supplied_foreground_job(42, "worker"))
    };
    pane.process_pty_bytes(pane_id, 7, b"\x1b]11;rgb:11/22", &tx, probe);
    assert_eq!(probes.get(), 0);
    pane.process_pty_bytes(pane_id, 7, b"/33\x07", &tx, probe);
    assert_eq!(probes.get(), 1);
    assert_eq!(
        pane.core.lock().unwrap().transient_default_color_owner_pgid,
        Some(42)
    );
    assert!(!pane.maybe_restore_host_terminal_theme(pane_id, 7, || None));
    assert!(
        !pane.maybe_restore_host_terminal_theme(pane_id, 7, || Some(supplied_foreground_job(
            42, "worker"
        )))
    );
    assert!(
        pane.maybe_restore_host_terminal_theme(pane_id, 7, || Some(supplied_foreground_job(
            7, "shell"
        )))
    );
    assert!(!pane.has_transient_default_color_override());
}

#[test]
fn process_pty_bytes_reports_latest_libghostty_pwd_callback() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let partial = pane.process_pty_bytes(pane_id, 0, b"\x1b]7;file:///tmp/herdr%20", &tx, |_| None);
    assert_eq!(partial.reported_cwd, None);

    let completed = pane.process_pty_bytes(pane_id, 0, b"repo\x07", &tx, |_| None);
    #[cfg(not(windows))]
    assert_eq!(
        completed.reported_cwd,
        Some(std::path::PathBuf::from("/tmp/herdr repo"))
    );
    #[cfg(windows)]
    assert_eq!(
        completed.reported_cwd,
        Some(std::path::PathBuf::from("\\tmp\\herdr repo"))
    );

    let latest = pane.process_pty_bytes(
        pane_id,
        0,
        b"\x1b]9;9;/tmp/conemu\x1b\\\x1b]1337;CurrentDir=/tmp/iterm2\x1b\\",
        &tx,
        |_| None,
    );
    assert_eq!(
        latest.reported_cwd,
        Some(std::path::PathBuf::from("/tmp/iterm2"))
    );
}

#[test]
fn process_pty_bytes_surfaces_live_bells_only() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.seed_history_ansi("stale\x07");
    let result = pane.process_pty_bytes(pane_id, 0, b"\x07\x1b]0;title\x07\x07", &tx, |_| None);

    assert_eq!(result.terminal_bells, 2);
    let drained = pane.process_pty_bytes(pane_id, 0, b"live output", &tx, |_| None);
    assert_eq!(drained.terminal_bells, 0);
}

#[test]
fn process_pty_bytes_reports_only_completed_title_changes() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    assert!(
        !pane
            .process_pty_bytes(pane_id, 0, b"\x1b]0;buil", &tx, |_| None)
            .terminal_title_changed
    );
    assert!(
        pane.process_pty_bytes(pane_id, 0, b"ding\x07", &tx, |_| None)
            .terminal_title_changed
    );
    assert!(
        !pane
            .process_pty_bytes(pane_id, 0, b"\x1b]2;building\x07", &tx, |_| None)
            .terminal_title_changed
    );
    assert!(
        pane.process_pty_bytes(pane_id, 0, b"\x1b]2;done\x07", &tx, |_| None)
            .terminal_title_changed
    );
}

#[test]
fn process_pty_bytes_surfaces_clipboard_writes_without_other_results() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();

    let result = pane.process_pty_bytes(
        PaneId::from_raw(1),
        0,
        b"output\x1b]52;c;Y2xpcGJvYXJk\x07",
        &tx,
        |_| None,
    );

    assert!(result.request_render);
    assert_eq!(result.render_delay, None);
    assert_eq!(result.terminal_bells, 0);
    assert_eq!(result.clipboard_writes, vec![b"clipboard".to_vec()]);
    assert_eq!(result.reported_cwd, None);
    assert!(result.terminal_responses.is_empty());
}

#[test]
fn seeded_history_clipboard_write_does_not_leak_into_live_output() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    pane.seed_history_ansi("\x1b]52;c;c3RhbGU=\x07");

    let result = pane.process_pty_bytes(PaneId::from_raw(1), 0, b"live output", &tx, |_| None);

    assert!(result.clipboard_writes.is_empty());
}

#[test]
fn seeded_history_pwd_does_not_leak_into_live_output() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    pane.seed_history_ansi("\x1b]7;file:///tmp/restored\x07");

    let result = pane.process_pty_bytes(PaneId::from_raw(1), 0, b"live output", &tx, |_| None);

    assert_eq!(result.reported_cwd, None);
}

#[cfg(windows)]
#[test]
fn windows_powershell_prompt_cwd_uses_latest_default_prompt_path() {
    let cwd = std::env::current_dir().unwrap();
    let text = format!("PS C:\\old> cd {}\nPS {}>", cwd.display(), cwd.display());

    assert_eq!(windows_powershell_prompt_cwd(&text).as_ref(), Some(&cwd));
}

#[cfg(windows)]
#[test]
fn windows_powershell_prompt_cwd_requires_current_prompt_line() {
    let cwd = std::env::current_dir().unwrap();
    let text = format!("PS {}>\ncommand output", cwd.display());

    assert_eq!(windows_powershell_prompt_cwd(&text), None);
}

#[cfg(windows)]
#[test]
fn windows_powershell_prompt_cwd_ignores_command_echo() {
    let cwd = std::env::current_dir().unwrap();
    let text = format!("PS {}> echo hi", cwd.display());

    assert_eq!(windows_powershell_prompt_cwd(&text), None);
}

#[cfg(windows)]
#[test]
fn process_pty_bytes_reports_windows_powershell_prompt_cwd() {
    let cwd = std::env::current_dir().unwrap();
    let bytes = format!("PS C:\\old> cd {}\r\nPS {}>", cwd.display(), cwd.display());

    let result = process_windows_powershell_prompt_bytes(bytes.as_bytes(), 80, 24, true);

    assert_eq!(result.reported_cwd.as_ref(), Some(&cwd));
}

#[cfg(windows)]
#[test]
fn process_pty_bytes_reports_wrapped_windows_powershell_prompt_cwd() {
    let cwd = std::env::current_dir().unwrap();
    let bytes = format!("PS {}>", cwd.display());

    let result = process_windows_powershell_prompt_bytes(bytes.as_bytes(), 12, 8, true);

    assert_eq!(result.reported_cwd.as_ref(), Some(&cwd));
}

#[cfg(windows)]
#[test]
fn process_pty_bytes_ignores_prompt_like_output_on_previous_line() {
    let cwd = std::env::current_dir().unwrap();
    let bytes = format!("PS {}>\r\n", cwd.display());

    let result = process_windows_powershell_prompt_bytes(bytes.as_bytes(), 80, 24, true);

    assert_eq!(result.reported_cwd, None);
}

#[cfg(windows)]
#[test]
fn process_pty_bytes_ignores_windows_powershell_prompt_cwd_on_alternate_screen() {
    let cwd = std::env::current_dir().unwrap();
    let bytes = format!("\x1b[?1049hPS {}>", cwd.display());

    let result = process_windows_powershell_prompt_bytes(bytes.as_bytes(), 80, 24, true);

    assert_eq!(result.reported_cwd, None);
}

#[cfg(windows)]
#[test]
fn process_pty_bytes_skips_windows_powershell_prompt_cwd_when_disabled() {
    let cwd = std::env::current_dir().unwrap();
    let bytes = format!("PS {}>", cwd.display());

    let result = process_windows_powershell_prompt_bytes(bytes.as_bytes(), 80, 24, false);

    assert_eq!(result.reported_cwd, None);
}

#[test]
fn synchronized_output_suppresses_intermediate_render_requests_until_batch_ends() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane_terminal = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let begin = pane_terminal.process_pty_bytes(pane_id, 0, b"\x1b[?2026h", &tx, |_| None);
    assert!(!begin.request_render);

    let body = pane_terminal.process_pty_bytes(pane_id, 0, b"hello", &tx, |_| None);
    assert!(!body.request_render);

    let end = pane_terminal.process_pty_bytes(pane_id, 0, b"\x1b[?2026l", &tx, |_| None);
    assert!(end.request_render);
}

#[test]
fn kitty_graphics_write_requests_render_with_settle_backstop() {
    crate::protocol::kitty::set_enabled(true);
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane_terminal = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane_terminal.process_pty_bytes(
        pane_id,
        0,
        b"\x1b_Ga=T,f=32,t=d,i=7,p=1,s=1,v=1,q=2;/wAA/w==\x1b\\",
        &tx,
        |_| None,
    );

    assert!(result.request_render);
    assert_eq!(result.render_delay, Some(KITTY_GRAPHICS_REDRAW_SETTLE));
}
