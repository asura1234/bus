use super::*;
use crate::terminal::runtime::SubmissionInspector;
use std::sync::{Arc, Mutex};

const OLD: &str = "old room message";
type Signal = Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>;

struct Watch {
    inner: Box<dyn SubmissionInspector>,
    signal: Signal,
}
impl SubmissionInspector for Watch {
    fn observe(&self, screen: &str) -> InputObservation {
        let result = self.inner.observe(screen);
        if let Some(signal) = self.signal.lock().unwrap().take() {
            let _ = signal.send(());
        }
        result
    }
    fn clearing(&self, index: usize) {
        self.inner.clearing(index);
    }
    fn cleanup_progress(&self) -> u64 {
        self.inner.cleanup_progress()
    }
}

async fn watched() -> (TerminalRuntime, tokio::sync::mpsc::Receiver<Bytes>, Signal) {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(123, 48, 32);
    runtime.test_process_pty_bytes(b"\x1b[?2004h");
    show(&runtime, AgentKind::Claude, "");
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let signal: Signal = Default::default();
    let inspector = Watch {
        inner: Box::new(observer(AgentKind::Claude, OLD.into())),
        signal: signal.clone(),
    };
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from(format!("\x1b[200~{OLD}\x1b[201~")),
            Bytes::from_static(b"\r"),
            [b"\x01", b"\x0b", b"\x15"]
                .map(|key| Bytes::copy_from_slice(key))
                .to_vec(),
            Duration::ZERO,
            None,
            Box::new(inspector),
        )
        .unwrap();
    assert!(writes.recv().await.unwrap().starts_with(b"\x1b[200~"));
    clock.wait_for_draft_wait();
    clock.advance(Duration::from_secs(3));
    assert!(completion.recv_timeout(WAIT).unwrap().is_err());
    (runtime, writes, signal)
}

async fn arm_and_wait(
    signal: &Signal,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    result: &mut tokio::task::JoinHandle<std::io::Result<()>>,
    ready: tokio::sync::oneshot::Receiver<()>,
) {
    let _ = signal;
    tokio::select! {
        ready = ready => ready.expect("old inspector dropped instead of retained"),
        bytes = writes.recv() => panic!("NEXT was pasted before the old paste was resolved: {bytes:?}"),
        result = result => panic!("NEXT refused before waiting for the old render: {result:?}"),
    }
    assert!(writes.try_recv().is_err());
}

async fn wait_for_late(partial: bool) {
    let (runtime, mut writes, signal) = watched().await;
    if partial {
        show(&runtime, AgentKind::Claude, "old room");
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    *signal.lock().unwrap() = Some(tx);
    let completion = send(&runtime, AgentKind::Claude, NEXT, None);
    let mut result = tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
    arm_and_wait(&signal, &mut writes, &mut result, rx).await;
    show(&runtime, AgentKind::Claude, OLD);
    finish_recovery(&runtime, &mut writes, AgentKind::Claude, OLD, false, result).await;
}

#[tokio::test]
async fn next_lease_waits_for_an_empty_composer_to_finish_the_old_render() {
    wait_for_late(false).await;
}

#[tokio::test]
async fn next_lease_waits_for_a_partial_old_render_to_finish() {
    wait_for_late(true).await;
}

#[tokio::test]
async fn a_retry_wait_expiry_does_not_forget_the_paste_that_can_still_land() {
    let (runtime, mut writes, signal) = watched().await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    *signal.lock().unwrap() = Some(tx);
    let completion = send(&runtime, AgentKind::Claude, NEXT, None);
    let mut result = tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
    arm_and_wait(&signal, &mut writes, &mut result, rx).await;
    runtime
        .test_submission_clock()
        .unwrap()
        .advance(Duration::from_secs(3));
    assert!(result.await.unwrap().is_err());
    assert!(writes.try_recv().is_err());
    // Still inside the generous record age; the late original paste must not
    // be mistaken for a user draft just because a retry's short wait expired.
    show(&runtime, AgentKind::Claude, OLD);
    recover_and_deliver(&runtime, &mut writes, AgentKind::Claude, OLD, false).await;
}

#[tokio::test]
async fn a_never_rendered_paste_expires_without_renewing_its_age_on_retries() {
    let (runtime, mut writes, _) = watched().await;
    runtime
        .test_submission_clock()
        .unwrap()
        .advance(Duration::from_secs(61));
    let completion = send(&runtime, AgentKind::Claude, NEXT, None);
    let mut result = tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
    assert_eq!(
        input_or_result(&mut writes, &mut result).await,
        Bytes::from(format!("\x1b[200~{NEXT}\x1b[201~"))
    );
    show(&runtime, AgentKind::Claude, NEXT);
    assert_eq!(
        input_or_result(&mut writes, &mut result).await.as_ref(),
        b"\r"
    );
    show(&runtime, AgentKind::Claude, "");
    assert!(result.await.unwrap().is_ok());
}

#[tokio::test]
async fn two_successive_uncertain_pastes_are_recovered_as_two_separate_owned_drafts() {
    let (runtime, mut writes, _) = watched().await;
    show(&runtime, AgentKind::Claude, OLD);
    recover_and_deliver(&runtime, &mut writes, AgentKind::Claude, OLD, false).await;
    uncertain(
        &runtime,
        &mut writes,
        AgentKind::Claude,
        "second old message",
        None,
        false,
    )
    .await;
    show(&runtime, AgentKind::Claude, "second old message");
    recover_and_deliver(
        &runtime,
        &mut writes,
        AgentKind::Claude,
        "second old message",
        false,
    )
    .await;
}

#[tokio::test]
async fn a_concatenation_or_user_change_is_never_cleared_as_a_single_owned_paste() {
    let (runtime, mut writes, signal) = watched().await;
    show(
        &runtime,
        AgentKind::Claude,
        "old room messagesecond old message",
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    *signal.lock().unwrap() = Some(tx);
    let completion = send(&runtime, AgentKind::Claude, NEXT, None);
    let mut result = tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
    arm_and_wait(&signal, &mut writes, &mut result, rx).await;
    runtime
        .test_submission_clock()
        .unwrap()
        .advance(Duration::from_secs(3));
    assert!(result.await.unwrap().is_err());
    assert!(
        writes.try_recv().is_err(),
        "no destructive guess about a concatenation"
    );
}

#[tokio::test]
async fn a_working_to_idle_boundary_releases_a_never_rendered_paste() {
    use crate::agents::AgentState;
    use crate::server::{
        app::{App, AppPolicy},
        workspaces::Workspace,
    };
    use crate::terminal::events::TerminalEvent;
    let (runtime, mut writes, _) = watched().await;
    let observed_at = runtime.test_submission_clock().unwrap().now();
    let (_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        AppPolicy::TEST,
        None,
        api_rx,
        crate::server::api::EventHub::default(),
    );
    app.state.workspaces = vec![Workspace::test_new("late paste")];
    app.state.ensure_test_terminals();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.state.insert_test_runtime(pane, runtime);
    for (n, state) in [AgentState::Working, AgentState::Idle]
        .into_iter()
        .enumerate()
    {
        app.handle_internal_event(TerminalEvent::StateChanged {
            pane_id: pane,
            agent: Some(AgentKind::Claude),
            state,
            visible_blocker: false,
            process_exited: false,
            exited_process: None,
            observed_at: observed_at + Duration::from_millis(n as u64 + 1),
        });
    }
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    let completion = send(runtime, AgentKind::Claude, NEXT, None);
    let mut result = tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
    assert!(input_or_result(&mut writes, &mut result)
        .await
        .starts_with(b"\x1b[200~"));
    show(runtime, AgentKind::Claude, NEXT);
    assert_eq!(
        input_or_result(&mut writes, &mut result).await.as_ref(),
        b"\r"
    );
    show(runtime, AgentKind::Claude, "");
    assert!(result.await.unwrap().is_ok());
}
