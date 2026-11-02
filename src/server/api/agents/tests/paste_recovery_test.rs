use super::owned_draft_cleanup_tests::{fail_custom_enter_receipt, finish_response, write_or_done};
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
    response: &mut (tokio::task::JoinHandle<T>, Option<T>),
) -> Bytes {
    match write_or_done(writes, response).await {
        Ok(bytes) => bytes,
        Err(result) => {
            panic!("submission completed before withdrawing its owned draft: {result:?}")
        }
    }
}

// The removed retry-only case's captured frame and exact-byte assertions live here.
#[tokio::test]
async fn claude_startup_collapsed_paste_submits_once_without_retyping() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let paste_receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, brief_method(method));
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from(format!("\x1b[200~{BRIEF}\x1b[201~"))
    );
    pasted(&app);
    assert!(
        response.try_recv().is_err(),
        "held write receipt is not completion"
    );
    assert!(
        writes.try_recv().is_err(),
        "no Enter before the paste receipt"
    );
    paste_receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    redraw(&app, AgentKind::Claude, "");
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
    assert!(
        writes.try_recv().is_err(),
        "one full paste and one Enter only"
    );
}

#[tokio::test]
async fn claude_unaccepted_collapsed_paste_is_cleared_before_uncertain_timeout() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let method = brief_method(method);
    let old_reply =
        fail_custom_enter_receipt(&mut app, method.clone(), BRIEF, &mut writes, pasted).await;
    // The next request recovers old owned input before sending its distinct payload.
    let Method::AgentPromptIfIdle(mut params) = method else {
        unreachable!()
    };
    params.text = "new message".into();
    let response = start(&mut app, Method::AgentPromptIfIdle(params));
    let mut response = (
        tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap()),
        None,
    );
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
    accept_paste(&app, AgentKind::Claude, "new message", &mut writes).await;
    let reply = finish_response(response).await;
    assert!(
        reply.contains("agent_prompted"),
        "fresh delivery after old cleanup: {reply}"
    );
    assert!(
        !old_reply.contains("agent_prompted") && !old_reply.contains("agent_prompt_not_shown"),
        "later cleanup cannot erase the old uncertainty: {old_reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "one fresh paste/Enter; no old replay or late cleanup"
    );
}

#[tokio::test]
async fn claude_draft_edit_during_withdrawal_is_preserved() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let old_reply =
        fail_custom_enter_receipt(&mut app, brief_method(method), BRIEF, &mut writes, pasted).await;
    let recovery = recovery_without_new_input(&app, None);
    let mut response = (
        tokio::task::spawn_blocking(move || recovery.recv_timeout(WAIT).unwrap()),
        None,
    );
    assert_eq!(
        cleanup_write(&mut writes, &mut response).await,
        Bytes::from_static(b"\x01")
    );
    redraw(&app, AgentKind::Claude, "developer's new draft");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let saved = app.lookup_runtime_sender(0, pane).unwrap().visible_ansi();
    let error = finish_response(response).await.unwrap_err();
    assert!(
        crate::terminal::runtime::PromptNotShown::composer_busy(&error),
        "old recovery must refuse the edited draft: {error}"
    );
    assert_eq!(
        app.lookup_runtime_sender(0, pane).unwrap().visible_ansi(),
        saved,
        "the exact developer draft must remain unchanged"
    );
    assert!(
        writes.try_recv().is_err(),
        "never kill, submit or replay an edited draft"
    );
    assert!(
        old_reply.contains("transcript and composer before resending"),
        "the old uncertain Enter still needs actionable guidance: {old_reply}"
    );
}

#[tokio::test]
async fn claude_withdrawal_stops_at_the_original_hard_deadline() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let method = brief_method(method);
    let hard_budget = crate::protocol::api::submission::method_budget(&method).unwrap();
    let expiry = confirmation_expiry(&method);
    let native_clock = clock(&app);
    let paste_receipt = native_clock.fail_next_write_completion(std::io::ErrorKind::TimedOut);
    let response = start(&mut app, method);
    let started = native_clock.now();
    let mut response = (
        tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap()),
        None,
    );
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from(format!("\x1b[200~{BRIEF}\x1b[201~"))
    );
    pasted(&app);
    let clear_receipt = native_clock.hold_next_write_completion();
    native_clock.advance(expiry);
    paste_receipt.send(()).unwrap();
    assert_eq!(
        cleanup_write(&mut writes, &mut response).await,
        Bytes::from_static(b"\x01")
    );
    // Ignore the clear key, and release its receipt at this transaction's original bound.
    native_clock.advance((started + hard_budget).saturating_duration_since(native_clock.now()));
    clear_receipt.send(()).unwrap();
    let reply = finish_response(response).await;
    assert!(reply.contains("cleanup could not finish safely"), "{reply}");
    assert!(
        !reply.contains("agent_prompted") && !reply.contains("agent_prompt_not_shown"),
        "failed cleanup remains uncertain: {reply}"
    );
    assert_eq!(
        native_clock.now(),
        started + hard_budget,
        "the original hard deadline was not extended"
    );
    assert!(
        writes.try_recv().is_err(),
        "no Enter or other write beyond the hard deadline"
    );
}

#[tokio::test]
async fn claude_paste_first_observed_at_expiry_is_withdrawn_without_enter() {
    use crate::terminal::runtime::InputObservation;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let (app, _, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let native_clock = clock(&app);
    native_clock.pause();
    let observed_clock = native_clock.clone();
    let first = Arc::new(AtomicBool::new(true));
    let observed_first = first.clone();
    let confirmation = crate::protocol::api::submission::confirmation_budget(Duration::ZERO);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let paste_receipt = native_clock.fail_next_write_completion(std::io::ErrorKind::TimedOut);
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
                    if observed_first.swap(false, Ordering::SeqCst) {
                        observed_clock.advance(confirmation);
                    }
                    InputObservation::Pending
                } else {
                    InputObservation::Empty
                }
            },
        )
        .unwrap();
    let mut response = (
        tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap()),
        None,
    );
    assert_eq!(next_write(&mut writes).await, Bytes::from(BRIEF));
    pasted(&app);
    paste_receipt.send(()).unwrap();
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
    let error = finish_response(response).await.unwrap_err();
    assert!(
        crate::terminal::runtime::PromptNotShown::is(&error),
        "{error}"
    );
    assert!(
        !first.load(Ordering::SeqCst),
        "the first owned observation crossed the actual confirmation boundary"
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
