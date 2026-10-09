use super::*;

#[tokio::test]
async fn cwd_returns_accepted_report_without_rechecking_filesystem() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let cwd = std::env::temp_dir().join(format!(
        "bus-reported-cwd-cache-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir(&cwd).expect("create reported cwd");

    let (runtime, _rx) = TerminalRuntime::test_with_channel(80, 24);
    let (events, _event_rx) = mpsc::channel(1);
    publish_reported_cwd(runtime.pane_id, cwd.clone(), &runtime.reported_cwd, &events);
    assert_eq!(
        runtime.reported_cwd.lock().unwrap().as_ref(),
        Some(&cwd),
        "test setup must pass cache admission"
    );

    std::fs::remove_dir(&cwd).expect("remove reported cwd after admission");

    assert_eq!(runtime.cwd(), Some(cwd));
}

#[cfg(unix)]
#[test]
fn process_cwd_does_not_require_traversing_the_directory_path() {
    use std::os::unix::fs::PermissionsExt;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let base = std::env::temp_dir().join(format!(
        "bus-process-cwd-no-stat-{}-{stamp}",
        std::process::id()
    ));
    let private = base.join("private");
    let cwd = private.join("cwd");
    std::fs::create_dir_all(&cwd).expect("create process cwd");

    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .current_dir(&cwd)
        .spawn()
        .expect("spawn process in cwd");
    let expected_cwd = crate::platform::process_cwd(child.id())
        .expect("resolve process cwd before restricting traversal");
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o000))
        .expect("make cwd path untraversable");

    let path_is_traversable = cwd.is_dir();
    let observed = (!path_is_traversable)
        .then(|| absolute_process_cwd(child.id()))
        .flatten();

    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755))
        .expect("restore cwd path permissions");
    let _ = child.kill();
    let _ = child.wait();
    std::fs::remove_dir_all(&base).expect("remove process cwd");

    if path_is_traversable {
        eprintln!("skipping untraversable cwd assertion for privileged test process");
        return;
    }
    assert_eq!(observed, Some(expected_cwd));
}

#[cfg(unix)]
#[tokio::test]
async fn follow_cwd_falls_back_to_reported_pane_cwd_without_foreground_group() {
    let (runtime, _rx) = TerminalRuntime::test_with_channel(80, 24);
    let cwd = std::env::temp_dir();
    *runtime.reported_cwd.lock().unwrap() = Some(cwd.clone());

    assert_eq!(runtime.follow_cwd(), Some(cwd));
}

#[tokio::test]
async fn focus_events_are_forwarded_when_enabled() {
    let (tx, mut rx) = mpsc::channel(4);
    let (resize_tx, _resize_rx) = watch::channel((80, 24, 0, 0));
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal
        .mode_set(crate::terminal::vt::MODE_FOCUS_EVENT, true)
        .unwrap();
    let pane_id = PaneId::from_raw(0);
    let terminal = Arc::new(PaneTerminal::new(
        GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap(),
    ));
    let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
    let runtime = TerminalRuntime {
        pane_id,
        terminal,
        io: PaneRuntimeIo::TestChannel {
            sender: tx,
            resize_tx,
        },
        current_size: Cell::new((80, 24, 0, 0)),
        child_pid: Arc::new(AtomicU32::new(0)),
        reported_cwd: Arc::new(Mutex::new(None)),
        child_wait_completed: None,
        kitty_keyboard_flags: Arc::new(AtomicU16::new(0)),
        content_seq: Arc::new(AtomicU64::new(0)),
        content_write_lock: Arc::new(Mutex::new(())),
        detection_content_seq: Arc::new(AtomicU64::new(0)),
        preserve_processes_on_drop: true,
        compression,
        detect_handle: Some(tokio::spawn(async {}).abort_handle()),
    };

    assert!(runtime.try_send_focus_event(crate::terminal::vt::FocusEvent::Gained));
    assert_eq!(rx.recv().await.unwrap(), Bytes::from_static(b"\x1b[I"));
}

#[tokio::test]
async fn focus_events_are_suppressed_when_disabled() {
    let (tx, mut rx) = mpsc::channel(4);
    let (resize_tx, _resize_rx) = watch::channel((80, 24, 0, 0));
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane_id = PaneId::from_raw(0);
    let terminal = Arc::new(PaneTerminal::new(
        GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap(),
    ));
    let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
    let runtime = TerminalRuntime {
        pane_id,
        terminal,
        io: PaneRuntimeIo::TestChannel {
            sender: tx,
            resize_tx,
        },
        current_size: Cell::new((80, 24, 0, 0)),
        child_pid: Arc::new(AtomicU32::new(0)),
        reported_cwd: Arc::new(Mutex::new(None)),
        child_wait_completed: None,
        kitty_keyboard_flags: Arc::new(AtomicU16::new(0)),
        content_seq: Arc::new(AtomicU64::new(0)),
        content_write_lock: Arc::new(Mutex::new(())),
        detection_content_seq: Arc::new(AtomicU64::new(0)),
        preserve_processes_on_drop: true,
        compression,
        detect_handle: Some(tokio::spawn(async {}).abort_handle()),
    };

    assert!(!runtime.try_send_focus_event(crate::terminal::vt::FocusEvent::Gained));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), rx.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn subscribed_idle_child_receives_color_scheme_transition() {
    let (runtime, mut rx) = TerminalRuntime::test_with_channel(80, 24);
    runtime.apply_host_terminal_appearance(Some(crate::utils::theme::color::HostAppearance::Dark));
    runtime.test_process_pty_bytes(b"\x1b[?2031h");

    runtime.apply_host_terminal_appearance(Some(crate::utils::theme::color::HostAppearance::Light));

    assert_eq!(rx.recv().await, Some(Bytes::from_static(b"\x1b[?997;2n")));
}

#[cfg(unix)]
#[tokio::test]
async fn spawned_pty_reader_aggregates_terminal_bells() {
    let (events, mut event_rx) = mpsc::channel(8);
    let pane_id = PaneId::from_raw(42);
    let runtime = TerminalRuntime::spawn_shell_command(
        pane_id,
        24,
        80,
        std::env::temp_dir(),
        "printf '\\a\\a'; sleep 0.05",
        &PaneLaunchEnv::default(),
        AgentDetection::Disabled,
        0,
        crate::utils::theme::color::TerminalTheme::default(),
        None,
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    )
    .unwrap();

    let bell = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(TerminalEvent::TerminalBell {
                pane_id: delivered_pane,
                count,
            }) = event_rx.recv().await
            {
                break (delivered_pane, count);
            }
        }
    })
    .await
    .expect("PTY reader should publish terminal bells");

    assert_eq!(bell, (pane_id, 2));
    runtime.shutdown();
}

#[tokio::test]
async fn state_changed_event_waits_for_queue_space_instead_of_dropping() {
    let (tx, mut rx) = mpsc::channel(1);
    let pane_id = PaneId::from_raw(42);

    tx.try_send(TerminalEvent::ClipboardWrite {
        content: Vec::new(),
    })
    .unwrap();

    let publish = publish_state_changed_event(
        tx.clone(),
        pane_id,
        Some(AgentKind::Pi),
        AgentState::Idle,
        false,
        false,
        std::time::Instant::now(),
    );
    tokio::pin!(publish);

    let blocked = tokio::time::timeout(std::time::Duration::from_millis(20), async {
        (&mut publish).await;
    })
    .await;
    assert!(
        blocked.is_err(),
        "publisher should wait for queue space instead of dropping StateChanged"
    );

    let first = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
        .await
        .expect("queue should yield first event")
        .expect("sender still alive");
    assert!(matches!(first, TerminalEvent::ClipboardWrite { .. }));

    tokio::time::timeout(std::time::Duration::from_millis(50), async {
        (&mut publish).await;
    })
    .await
    .expect("publisher should complete once queue space is available");

    let second = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
        .await
        .expect("queue should yield second event")
        .expect("sender still alive");
    assert!(matches!(
        second,
        TerminalEvent::StateChanged {
            pane_id: delivered_pane,
            agent: Some(AgentKind::Pi),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: false,
            observed_at: _,
        } if delivered_pane == pane_id
    ));
}
