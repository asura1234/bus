use super::*;

#[test]
fn process_pty_bytes_reports_latest_libghostty_pwd_callback() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let partial = pane.process_pty_bytes(pane_id, 0, b"\x1b]7;file:///tmp/herdr%20", &tx);
    assert_eq!(partial.reported_cwd, None);

    let completed = pane.process_pty_bytes(pane_id, 0, b"repo\x07", &tx);
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
    );
    assert_eq!(
        latest.reported_cwd,
        Some(std::path::PathBuf::from("/tmp/iterm2"))
    );
}

#[test]
fn process_pty_bytes_surfaces_live_bells_only() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.seed_history_ansi("stale\x07");
    let result = pane.process_pty_bytes(pane_id, 0, b"\x07\x1b]0;title\x07\x07", &tx);

    assert_eq!(result.terminal_bells, 2);
    let drained = pane.process_pty_bytes(pane_id, 0, b"live output", &tx);
    assert_eq!(drained.terminal_bells, 0);
}

#[test]
fn process_pty_bytes_reports_only_completed_title_changes() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    assert!(
        !pane
            .process_pty_bytes(pane_id, 0, b"\x1b]0;buil", &tx)
            .terminal_title_changed
    );
    assert!(
        pane.process_pty_bytes(pane_id, 0, b"ding\x07", &tx)
            .terminal_title_changed
    );
    assert!(
        !pane
            .process_pty_bytes(pane_id, 0, b"\x1b]2;building\x07", &tx)
            .terminal_title_changed
    );
    assert!(
        pane.process_pty_bytes(pane_id, 0, b"\x1b]2;done\x07", &tx)
            .terminal_title_changed
    );
}

#[test]
fn process_pty_bytes_surfaces_clipboard_writes_without_other_results() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();

    let result = pane.process_pty_bytes(
        PaneId::from_raw(1),
        0,
        b"output\x1b]52;c;Y2xpcGJvYXJk\x07",
        &tx,
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
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    pane.seed_history_ansi("\x1b]52;c;c3RhbGU=\x07");

    let result = pane.process_pty_bytes(PaneId::from_raw(1), 0, b"live output", &tx);

    assert!(result.clipboard_writes.is_empty());
}

#[test]
fn seeded_history_pwd_does_not_leak_into_live_output() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(80, 24, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    pane.seed_history_ansi("\x1b]7;file:///tmp/restored\x07");

    let result = pane.process_pty_bytes(PaneId::from_raw(1), 0, b"live output", &tx);

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
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane_terminal = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let begin = pane_terminal.process_pty_bytes(pane_id, 0, b"\x1b[?2026h", &tx);
    assert!(!begin.request_render);

    let body = pane_terminal.process_pty_bytes(pane_id, 0, b"hello", &tx);
    assert!(!body.request_render);

    let end = pane_terminal.process_pty_bytes(pane_id, 0, b"\x1b[?2026l", &tx);
    assert!(end.request_render);
}

#[test]
fn kitty_graphics_write_requests_render_with_settle_backstop() {
    crate::kitty_graphics::set_enabled(true);
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane_terminal = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane_terminal.process_pty_bytes(
        pane_id,
        0,
        b"\x1b_Ga=T,f=32,t=d,i=7,p=1,s=1,v=1,q=2;/wAA/w==\x1b\\",
        &tx,
    );

    assert!(result.request_render);
    assert_eq!(result.render_delay, Some(KITTY_GRAPHICS_REDRAW_SETTLE));
}
