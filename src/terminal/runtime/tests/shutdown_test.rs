use super::*;

#[test]
fn shutdown_liveness_treats_reaped_direct_child_as_gone() {
    assert!(!process_alive_for_shutdown(42, 42, true, |_| true));
}

#[cfg(unix)]
#[tokio::test]
async fn checked_close_stops_owned_native_session_and_clears_pid_before_retry() {
    let (events, _event_rx) = mpsc::channel(8);
    let runtime = TerminalRuntime::spawn_shell_command(
        PaneId::from_raw(43),
        24,
        80,
        std::env::temp_dir(),
        "exec sleep 30",
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
    let pid = runtime.child_pid.load(Ordering::Acquire);
    assert_ne!(pid, 0);
    assert!(runtime.stop_session_for_close());
    assert_eq!(runtime.child_pid.load(Ordering::Acquire), 0);
    assert!(runtime.stop_session_for_close());
    runtime.shutdown();
}

#[test]
fn shutdown_liveness_keeps_unreaped_direct_child_alive() {
    assert!(process_alive_for_shutdown(42, 42, false, |_| true));
}

#[test]
fn shutdown_liveness_keeps_other_session_processes_alive() {
    assert!(process_alive_for_shutdown(43, 42, true, |_| true));
}

#[test]
fn shutdown_liveness_treats_missing_process_as_gone() {
    assert!(!process_alive_for_shutdown(43, 42, false, |_| false));
}

#[cfg(unix)]
#[tokio::test]
async fn codexup_close_reaps_installer_after_its_session_leader_exits() {
    let marker = std::env::temp_dir().join(format!(
        "bus-orphan-{}-{}",
        std::process::id(),
        crate::utils::time::now_ns()
    ));
    let script = format!("import os,signal,time; signal.signal(signal.SIGHUP,signal.SIG_IGN); signal.signal(signal.SIGINT,signal.SIG_IGN); p=os.fork(); open({:?},'w').write(str(os.getpid())) if p==0 else None; time.sleep(30) if p==0 else None", marker.to_string_lossy());
    let command = format!("exec python3 -c '{}'", script.replace('\'', "'\\''"));
    let (events, _rx) = mpsc::channel(8);
    let runtime = TerminalRuntime::spawn_shell_command(
        PaneId::from_raw(44),
        24,
        80,
        std::env::temp_dir(),
        &command,
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let child = loop {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            if let Ok(pid) = text.parse::<u32>() {
                break pid;
            }
        }
        assert!(std::time::Instant::now() < deadline, "orphan did not start");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    while !runtime
        .child_wait_completed
        .as_ref()
        .unwrap()
        .load(Ordering::Acquire)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "session leader did not exit"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let stopped = runtime.stop_session_for_close();
    let escaped = crate::platform::process_exists(child);
    // Always clean the harmless scratch child, also on the red run.
    crate::platform::signal_processes(&[child], crate::platform::Signal::Kill);
    let _ = std::fs::remove_file(marker);
    runtime.shutdown();
    assert!(stopped);
    assert!(
        !escaped,
        "pane close reported success but orphan installer {child} survived"
    );
}
