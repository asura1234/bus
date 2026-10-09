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
