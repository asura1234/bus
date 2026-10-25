use super::*;

const BRIEF: &str = "bus master (room 0):\nReview the change.\nRead the prior findings.\nUse the author worktree.\nRun focused tests.\nReport your verdict.";
const PASTED: &str = include_str!("fixtures/claude_startup_paste.txt");

fn brief_method(mut method: Method) -> Method {
    let Method::AgentPromptIfIdle(params) = &mut method else {
        panic!("expected guarded Claude submission");
    };
    params.text = BRIEF.into();
    method
}

fn pasted(app: &App) {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    runtime.resize(48, 122, 0, 0);
    runtime.test_process_pty_bytes(
        format!("\x1b[2J\x1b[H{}", PASTED.replace('\n', "\r\n")).as_bytes(),
    );
}

async fn cleanup_write<T: std::fmt::Debug>(
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    response: &mut tokio::task::JoinHandle<T>,
) -> Bytes {
    tokio::select! {
        bytes = next_write(writes) => bytes,
        result = response => panic!("submission completed before withdrawing its owned draft: {result:?}"),
    }
}

#[tokio::test]
async fn claude_startup_collapsed_paste_retries_only_enter_and_confirms_acceptance() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let response = start(&mut app, brief_method(method));
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from(format!("\x1b[200~{BRIEF}\x1b[201~"))
    );
    pasted(&app);
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).wait_for_enters(1);
    clock(&app).advance(Duration::from_millis(250));
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    assert!(
        response.try_recv().is_err(),
        "Enter write is not acceptance"
    );
    redraw(&app, AgentKind::Claude, "");
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
    assert!(writes.try_recv().is_err(), "one paste only");
}

#[tokio::test]
async fn claude_unaccepted_collapsed_paste_is_cleared_before_uncertain_timeout() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let expiry = confirmation_expiry(&method);
    let response = start(&mut app, brief_method(method.clone()));
    let mut response = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from(format!("\x1b[200~{BRIEF}\x1b[201~"))
    );
    pasted(&app);
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).wait_for_enters(1);
    // The fake provider keeps the exact saved placeholder despite Enter.
    clock(&app).advance(expiry);
    assert_eq!(
        cleanup_write(&mut writes, &mut response).await,
        Bytes::from_static(b"\x01")
    );
    pasted(&app);
    assert_eq!(
        cleanup_write(&mut writes, &mut response).await,
        Bytes::from_static(b"\x0b")
    );
    redraw(&app, AgentKind::Claude, "");
    let response = response.await.unwrap();
    assert!(response.contains("timeout"), "{response}");
    assert!(response.contains("draft was cleared"), "{response}");
    assert!(!response.contains("agent_prompted"));
    assert!(
        !response.contains("agent_prompt_not_shown"),
        "Enter may have run the message"
    );
    assert!(writes.try_recv().is_err(), "no late Enter or second paste");
    // A subsequent delivery starts with its own paste, never the old brief.
    let (tx, rx) = std::sync::mpsc::channel();
    let Method::AgentPromptIfIdle(mut params) = method else {
        unreachable!()
    };
    params.text = "new message".into();
    assert!(app.handle_deferred_agent_api_request(
        Request {
            id: "followup".into(),
            method: Method::AgentPromptIfIdle(params)
        },
        tx
    ));
    accept_paste(&app, AgentKind::Claude, "new message", &mut writes).await;
    assert!(rx.recv_timeout(WAIT).unwrap().contains("agent_prompted"));
}

#[tokio::test]
async fn claude_draft_edit_during_withdrawal_is_preserved() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let expiry = confirmation_expiry(&method);
    let response = start(&mut app, brief_method(method));
    let mut response = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
    next_write(&mut writes).await;
    pasted(&app);
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).wait_for_enters(1);
    clock(&app).advance(expiry);
    assert_eq!(
        cleanup_write(&mut writes, &mut response).await,
        Bytes::from_static(b"\x01")
    );
    redraw(&app, AgentKind::Claude, "developer's new draft");
    let response = response.await.unwrap();
    assert!(
        response.contains("could not be safely cleared"),
        "{response}"
    );
    assert!(
        response.contains("transcript and composer before resending"),
        "{response}"
    );
    assert!(
        writes.try_recv().is_err(),
        "never kill or submit an edited draft"
    );
}

#[tokio::test]
async fn claude_withdrawal_stops_at_the_original_hard_deadline() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let expiry = confirmation_expiry(&method);
    let response = start(&mut app, brief_method(method));
    let mut response = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
    next_write(&mut writes).await;
    pasted(&app);
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).wait_for_enters(1);
    clock(&app).advance(expiry);
    assert_eq!(
        cleanup_write(&mut writes, &mut response).await,
        Bytes::from_static(b"\x01")
    );
    // The provider also ignores the clear key. Exhaust the reserved second.
    clock(&app).advance(crate::protocol::api::submission::WITHDRAW_GRACE);
    let response = response.await.unwrap();
    assert!(
        response.contains("could not be safely cleared"),
        "{response}"
    );
    assert!(!response.contains("agent_prompted"));
    assert!(
        writes.try_recv().is_err(),
        "no writes beyond the hard deadline"
    );
}

#[tokio::test]
async fn claude_paste_first_observed_at_expiry_is_withdrawn_without_enter() {
    use crate::terminal::runtime::InputObservation;
    use std::sync::atomic::{AtomicBool, Ordering};
    let (app, _, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let clock = clock(&app);
    clock.pause();
    let observed_clock = clock.clone();
    let first = AtomicBool::new(true);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let completion = app
        .lookup_runtime_sender(0, pane)
        .unwrap()
        .queue_observed_input_submission(
            Bytes::from(BRIEF),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            None,
            move |screen| {
                if screen.contains("[Pasted text #1 +5 lines]") {
                    if first.swap(false, Ordering::SeqCst) {
                        // Simulate rendering/lock latency crossing the deadline.
                        observed_clock.advance(Duration::from_millis(2100));
                    }
                    InputObservation::Pending
                } else {
                    InputObservation::Empty
                }
            },
        )
        .unwrap();
    let mut response = tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
    assert_eq!(next_write(&mut writes).await, Bytes::from(BRIEF));
    pasted(&app);
    assert_eq!(
        cleanup_write(&mut writes, &mut response).await,
        Bytes::from_static(b"\x01")
    );
    pasted(&app);
    assert_eq!(
        cleanup_write(&mut writes, &mut response).await,
        Bytes::from_static(b"\x0b")
    );
    redraw(&app, AgentKind::Claude, "");
    let error = response.await.unwrap().unwrap_err();
    assert!(
        crate::terminal::runtime::PromptNotShown::is(&error),
        "{error}"
    );
    assert!(writes.try_recv().is_err(), "no Enter or late paste");
}

#[tokio::test]
async fn claude_launch_pending_refuses_input_then_accepts_the_brief_when_ready() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    let now = std::time::Instant::now();
    app.state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .begin_managed_agent(
            "bus-r1-a2".into(),
            AgentKind::Claude,
            now,
            Duration::from_secs(3600),
            Duration::from_secs(7200),
        );
    let response = start(&mut app, brief_method(method.clone()))
        .recv_timeout(WAIT)
        .unwrap();
    assert!(response.contains("agent_not_ready"), "{response}");
    assert!(writes.try_recv().is_err(), "no input while launching");
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    terminal.set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(3601), false);
    let response = start(&mut app, brief_method(method));
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from(format!("\x1b[200~{BRIEF}\x1b[201~"))
    );
    pasted(&app);
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    redraw(&app, AgentKind::Claude, "");
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
}
