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
async fn close_reaps_installer_after_its_session_leader_exits() {
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
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let escaped = pane_fixture_is_executing(child);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn run_pane_shutdown_fixture(script: &str) -> (TerminalRuntime, std::path::PathBuf, u32) {
    use std::time::{Duration, Instant};

    let marker = crate::utils::test_temp::unique_temp_path("bus-pane-shutdown");
    let (events, _event_rx) = mpsc::channel(8);
    let runtime = TerminalRuntime::spawn_argv_command(
        PaneId::from_raw(45),
        24,
        80,
        std::env::temp_dir(),
        &[
            "python3".into(),
            "-c".into(),
            script.into(),
            marker.to_str().unwrap().into(),
        ],
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

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            if let Some(pid) = text.strip_suffix('\n').and_then(|pid| pid.parse().ok()) {
                return (runtime, marker, pid);
            }
        }
        assert!(Instant::now() < deadline, "shutdown fixture did not start");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn shutdown_kills_term_ignoring_grandchild_that_left_the_pty_session() {
    use std::time::{Duration, Instant};

    let (runtime, marker, grandchild) = run_pane_shutdown_fixture(
        r#"
import os, signal, sys, time

if os.fork() == 0:
    if os.fork() == 0:
        os.setsid()
        signal.signal(signal.SIGHUP, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        with open(sys.argv[1], "w") as ready:
            print(os.getpid(), file=ready)
time.sleep(15)
"#,
    )
    .await;
    let original_group = runtime.child_pid.load(Ordering::Acquire);
    assert!(!crate::platform::session_processes(original_group).contains(&grandchild));

    let started = Instant::now();
    let stopped = runtime.stop_session_for_close();
    let elapsed = started.elapsed();
    let escaped = pane_fixture_is_executing(grandchild);
    // Clean up the fixture on both sides of the regression.
    if escaped {
        crate::platform::signal_processes(&[grandchild], crate::platform::Signal::Kill);
    }
    let _ = std::fs::remove_file(marker);
    runtime.shutdown();

    assert!(
        !escaped,
        "detached grandchild {grandchild} survived pane shutdown"
    );
    assert!(stopped, "checked shutdown did not confirm the tree exited");
    assert!(
        elapsed < Duration::from_secs(2),
        "checked shutdown took {elapsed:?}"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn pane_fixture_is_executing(pid: u32) -> bool {
    // A grandchild can already be dead while its new parent has yet to wait.
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .unwrap();
    let status = std::str::from_utf8(&output.stdout).unwrap().trim();
    !status.is_empty() && !status.starts_with('Z')
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn assert_pane_shutdown_allows_hangup_handler_to_finish(delay_seconds: &str) {
    let script = format!(
        r#"
import os, signal, sys, time

def hangup(_signal, _frame):
    time.sleep({delay_seconds})
    with open(sys.argv[1], "a") as ready:
        print("hangup completed", file=ready)
    sys.exit(0)

signal.signal(signal.SIGHUP, hangup)
with open(sys.argv[1], "w") as ready:
    print(os.getpid(), file=ready)
time.sleep(15)
"#
    );
    let (runtime, marker, pid) = run_pane_shutdown_fixture(&script).await;
    let stopped = runtime.stop_session_for_close();
    let observed = std::fs::read_to_string(&marker).unwrap();
    let _ = std::fs::remove_file(marker);
    runtime.shutdown();
    assert!(stopped);
    assert_eq!(observed, format!("{pid}\nhangup completed\n"));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn pane_shutdown_allows_a_cooperative_child_to_exit() {
    assert_pane_shutdown_allows_hangup_handler_to_finish("0").await;
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[tokio::test]
async fn pane_shutdown_allows_child_to_finish_within_hangup_grace() {
    assert_pane_shutdown_allows_hangup_handler_to_finish("0.08").await;
}
