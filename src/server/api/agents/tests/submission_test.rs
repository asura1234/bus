use super::*;
use crate::protocol::api::schema::{
    AgentPromptIfIdleParams, AgentPromptIfUnboundParams, Method, Request,
};

pub(super) const WAIT: Duration = crate::terminal::runtime::NATIVE_TEST_TIMEOUT;

fn clock(app: &App) -> crate::terminal::runtime::SubmissionClock {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_submission_clock()
        .unwrap()
}

fn confirmation_expiry(method: &Method) -> Duration {
    use crate::protocol::api::submission::{method_budget, WITHDRAW_GRACE};
    // Provider paste delay varies on ConPTY. Keep the withdrawal reserve
    // while advancing beyond the declared confirmation window.
    method_budget(method).unwrap() - WITHDRAW_GRACE + Duration::from_millis(100)
}

fn provider_fixture(
    kind: AgentKind,
    bound: bool,
    draft: &str,
) -> (App, Method, tokio::sync::mpsc::Receiver<Bytes>) {
    let mut app = app_with_agent();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
        .attached_terminal_id
        .clone();
    let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
    let now = std::time::Instant::now();
    terminal.begin_managed_agent(
        "bus-r1-a2".into(),
        kind,
        now,
        Duration::ZERO,
        Duration::from_secs(10),
    );
    terminal.set_detected_state(Some(kind), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(1), false);
    let label = crate::agents::agent_label(kind);
    if bound {
        terminal.set_agent_session_ref_for_session_start(
            "bus".into(),
            label.into(),
            crate::agents::resume::catalog::AgentSessionRef::id("session"),
            Some(1),
            None,
            &[],
        );
    }
    let (runtime, writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_process_pty_bytes(b"\x1b[?2004h");
    runtime.test_process_pty_bytes(screen(kind, draft).as_bytes());
    app.state.insert_test_runtime(pane, runtime);
    let info = app.agent_info(0, pane).unwrap();
    let method = if bound {
        Method::AgentPromptIfIdle(AgentPromptIfIdleParams {
            target: info.pane_id.clone(),
            text: "from bus".into(),
            expected_terminal_id: info.terminal_id,
            expected_pane_id: info.pane_id,
            expected_agent: label.into(),
            expected_session_id: "session".into(),
            steer: false,
        })
    } else {
        Method::AgentPromptIfUnbound(AgentPromptIfUnboundParams {
            target: info.pane_id.clone(),
            text: "from bus".into(),
            expected_terminal_id: info.terminal_id,
            expected_pane_id: info.pane_id,
            expected_managed_name: "bus-r1-a2".into(),
        })
    };
    (app, method, writes)
}

fn screen(kind: AgentKind, draft: &str) -> String {
    let body = match kind {
        AgentKind::Claude => {
            format!("────────────────────────\r\n❯ {draft}\r\n────────────────────────")
        }
        AgentKind::Cursor => format!(" > {draft}\r\n / commands · @ files"),
        _ => format!("› {draft}\r\n\r\n GPT-6 · /repo\r\n ? for shortcuts"),
    };
    format!("\x1b[2J\x1b[H{body}")
}

#[tokio::test]
async fn an_expired_dispatch_budget_is_definitely_rejected_before_any_input() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let (respond_to, response) = std::sync::mpsc::channel();
    // Inject queue/drain latency without sleeping: 4 s has consumed the
    // native budget, but the coordinator's 4.3 s read is still open.
    assert!(
        app.handle_deferred_agent_api_message(crate::server::api::ApiRequestMessage {
            received_at: std::time::Instant::now() - Duration::from_secs(4),
            request: Request {
                id: "delayed_dispatch".into(),
                method
            },
            respond_to,
        })
    );
    let value: serde_json::Value = serde_json::from_str(
        &response
            .recv_timeout(Duration::from_millis(100))
            .expect("exhausted dispatch budget must answer immediately"),
    )
    .unwrap();
    assert_eq!(
        value["error"]["code"],
        crate::protocol::api::AGENT_PROMPT_NOT_SHOWN
    );
    assert!(
        writes.try_recv().is_err(),
        "expired request must not type or press Enter"
    );
}

#[tokio::test]
async fn a_definite_withdrawal_after_the_writer_deadline_reaches_the_response() {
    let (app, _, _) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let agent = app.agent_info(0, pane).unwrap();
    let writer_deadline = std::time::Instant::now();
    let observed_at = writer_deadline + Duration::from_millis(20);
    let result_delay = Duration::from_millis(50);
    let response = crate::server::api::agents::prompt::complete_agent_prompt(
        "late_withdrawal".into(),
        agent,
        writer_deadline,
        writer_deadline - Duration::from_secs(3),
        observed_at,
        |timeout| {
            if timeout >= result_delay {
                Ok(Err(std::io::Error::other(
                    crate::terminal::runtime::PromptNotShown,
                )))
            } else {
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            }
        },
    );
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(
        response["error"]["code"],
        crate::protocol::api::AGENT_PROMPT_NOT_SHOWN
    );
}

#[tokio::test]
async fn an_expired_native_budget_answers_without_typing_or_waiting_for_a_new_deadline() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let budget = crate::protocol::api::submission::method_budget(&method).unwrap();
    let started = std::time::Instant::now() - budget - Duration::from_secs(1);
    let (tx, rx) = std::sync::mpsc::channel();
    assert!(app.handle_deferred_agent_api_request_at(
        Request {
            id: "expired".into(),
            method
        },
        tx,
        started,
    ));
    let response: serde_json::Value = serde_json::from_str(
        &rx.recv_timeout(WAIT)
            .expect("native response must be bounded"),
    )
    .unwrap();
    assert_eq!(
        response["error"]["code"],
        crate::protocol::api::AGENT_PROMPT_NOT_SHOWN
    );
    assert!(
        writes.try_recv().is_err(),
        "expired work must not type or press Enter"
    );
}

#[tokio::test]
async fn an_unresponsive_writer_still_returns_uncertain_timeout() {
    let (app, _, _) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let agent = app.agent_info(0, pane).unwrap();
    let deadline = std::time::Instant::now();
    let response = crate::server::api::agents::prompt::complete_agent_prompt(
        "unresponsive_writer".into(),
        agent,
        deadline,
        deadline,
        deadline,
        |_| Err(std::sync::mpsc::RecvTimeoutError::Timeout),
    );
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["error"]["code"], "timeout");
}

#[tokio::test]
async fn guarded_submission_rejects_unavailable_composer_before_any_write() {
    let (app, _, mut writes) = provider_fixture(AgentKind::Codex, false, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    runtime.test_submission_clock().unwrap().pause();
    let completion = runtime
        .queue_observed_input_submission(
            Bytes::from_static(b"from bus"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01")],
            Duration::ZERO,
            None,
            |_| crate::terminal::runtime::InputObservation::Unavailable,
        )
        .unwrap();
    let error = completion.recv_timeout(WAIT).unwrap().unwrap_err();
    assert!(
        crate::terminal::runtime::PromptNotShown::is(&error),
        "{error}"
    );
    assert!(
        writes.try_recv().is_err(),
        "no editing keys, paste or Enter"
    );
}

pub(super) fn redraw(app: &App, kind: AgentKind, draft: &str) {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(screen(kind, draft).as_bytes());
}

#[tokio::test]
async fn startup_controls_appearing_before_clear_are_definite_non_delivery() {
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_submission_clock().unwrap().pause();
    runtime.test_process_pty_bytes(screen(AgentKind::Codex, "").as_bytes());
    let runtime = std::sync::Arc::new(std::sync::Mutex::new(runtime));
    let runtime_for_redraw = runtime.clone();
    let first = std::sync::atomic::AtomicBool::new(true);
    let completion = runtime.lock().unwrap()
        .queue_observed_input_submission(
            Bytes::from_static(b"from bus"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01")],
            Duration::ZERO,
            None,
            move |snapshot| {
                let observation = if snapshot.contains("Hooks need review") {
                    crate::terminal::runtime::InputObservation::Unavailable
                } else {
                    assert!(snapshot.contains('›'), "expected real empty composer: {snapshot}");
                    crate::terminal::runtime::InputObservation::Empty
                };
                if first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    runtime_for_redraw.lock().unwrap().test_process_pty_bytes(
                        b"\x1b[2J\x1b[HHooks need review\r\n3 hooks are new or changed.\r\n\xe2\x80\xba 1. Review hooks\r\n2. Trust all and continue\r\n3. Continue without trusting (hooks won't run)\r\nPress enter to confirm or esc to go back\r\n",
                    );
                }
                observation
            },
        )
        .unwrap();
    let error = completion.recv_timeout(WAIT).unwrap().unwrap_err();
    assert!(writes.try_recv().is_err(), "startup control received input");
    assert!(
        crate::terminal::runtime::PromptNotShown::is(&error),
        "a refusal before any write must retain the queued message: {error}"
    );
}

pub(super) fn start(app: &mut App, method: Method) -> std::sync::mpsc::Receiver<String> {
    if let Some(clock) = app.test_prompt_clock(&method) {
        clock.pause();
    }
    let (tx, rx) = std::sync::mpsc::channel();
    assert!(app.handle_deferred_agent_api_request(
        Request {
            id: "reliability".into(),
            method
        },
        tx
    ));
    rx
}

pub(super) async fn next_write(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    tokio::time::timeout(WAIT, writes.recv())
        .await
        .expect("expected native input write")
        .expect("native input channel closed")
}

#[tokio::test]
async fn fake_provider_scheduling_pause_does_not_expire_a_submission() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let response = start(&mut app, method);
    // Model the fixture being descheduled during another build's test burst.
    // This is an injected scheduling stall, not a provider readiness wait.
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        response.try_recv().is_err(),
        "fixture scheduling must not consume the native submission clock"
    );
    accept_paste(&app, AgentKind::Codex, "from bus", &mut writes).await;
    let response = response.recv_timeout(WAIT).unwrap();
    assert!(response.contains("agent_prompted"), "{response}");
}

#[tokio::test]
async fn ignored_first_enter_is_verified_and_retried_without_repasting() {
    for (kind, bound) in [
        (AgentKind::Codex, true),
        (AgentKind::Codex, false),
        (AgentKind::Claude, true),
        (AgentKind::Cursor, true),
    ] {
        let (mut app, method, mut writes) = provider_fixture(kind, bound, "");
        let response = start(&mut app, method);
        assert_eq!(
            next_write(&mut writes).await,
            Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
        );
        redraw(&app, kind, "from bus");
        assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
        clock(&app).wait_for_enters(1);
        clock(&app).advance(Duration::from_millis(250));
        assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
        assert!(
            response.try_recv().is_err(),
            "{kind:?}: PTY write is not provider acceptance"
        );
        redraw(&app, kind, "");
        let response = response.recv_timeout(WAIT).unwrap();
        assert!(response.contains("agent_prompted"), "{kind:?}: {response}");
        assert!(writes.try_recv().is_err(), "must not type the prompt twice");
    }
}

#[tokio::test]
async fn stranded_draft_is_cleared_before_guarded_delivery_for_every_provider() {
    for (kind, bound) in [
        (AgentKind::Codex, true),
        (AgentKind::Codex, false),
        (AgentKind::Claude, true),
        (AgentKind::Cursor, true),
    ] {
        stranded_draft(kind, bound).await;
    }
}

pub(super) async fn accept_paste(
    app: &App,
    kind: AgentKind,
    text: &str,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
) {
    assert_eq!(
        next_write(writes).await,
        Bytes::from(format!("\x1b[200~{text}\x1b[201~"))
    );
    redraw(app, kind, text);
    assert_eq!(next_write(writes).await, Bytes::from_static(b"\r"));
    redraw(app, kind, "");
}

pub(super) async fn stranded_draft(kind: AgentKind, bound: bool) {
    let (mut app, method, mut writes) = provider_fixture(kind, bound, "stranded draft");
    let response = start(&mut app, method);
    // Ink-based providers can decode a burst of control characters as one
    // event. Send each editing key separately and allow a redraw between them.
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\x01"));
    redraw(&app, kind, "stranded draft");
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\x0b"));
    redraw(&app, kind, "");
    accept_paste(&app, kind, "from bus", &mut writes).await;
    let response = response.recv_timeout(WAIT).unwrap();
    assert!(response.contains("agent_prompted"), "{kind:?}: {response}");
}

#[tokio::test]
async fn delayed_paste_render_waits_before_the_first_enter() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, false, "");
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    clock(&app).advance(Duration::from_millis(450));
    clock(&app).wait_for_draft_wait();
    assert!(
        writes.try_recv().is_err(),
        "Enter must wait until provider consumed paste"
    );
    redraw(&app, AgentKind::Codex, "from bus");
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    redraw(&app, AgentKind::Codex, "");
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
}

#[tokio::test]
async fn changed_draft_after_enter_is_never_submitted_by_retry() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let expiry = confirmation_expiry(&method);
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    redraw(&app, AgentKind::Codex, "from bus");
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).wait_for_enters(1);
    redraw(&app, AgentKind::Codex, "frombus");
    let generation = clock(&app).advance(Duration::from_millis(250));
    clock(&app).wait_for_submission_check(generation);
    assert!(
        writes.try_recv().is_err(),
        "do not retry Enter on someone else's draft when the retry is due"
    );
    clock(&app).advance(expiry);
    let response = response.recv_timeout(WAIT).unwrap();
    assert!(response.contains("timeout"), "{response}");
    assert!(!response.contains("agent_prompted"));
    assert!(
        writes.try_recv().is_err(),
        "do not retry Enter on someone else's draft"
    );
}

#[tokio::test]
async fn enter_never_accepted_times_out_without_repasting_or_reporting_success() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Cursor, true, "");
    let expiry = confirmation_expiry(&method);
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    redraw(&app, AgentKind::Cursor, "from bus");
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).wait_for_enters(1);
    clock(&app).advance(Duration::from_millis(250));
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).advance(expiry);
    let response = response.recv_timeout(WAIT).unwrap();
    assert!(response.contains("timeout"), "{response}");
    assert!(!response.contains("agent_prompted"));
    while let Ok(bytes) = writes.try_recv() {
        assert_eq!(bytes, Bytes::from_static(b"\r"));
    }
}

#[tokio::test]
async fn an_unrecognised_paste_is_cleared_and_reported_as_not_submitted() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let expiry = confirmation_expiry(&method);
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    // A rendering the observer cannot attribute to Bus: never press Enter on it.
    redraw(&app, AgentKind::Claude, "[Pasted text #3] from bus?");
    clock(&app).advance(expiry);
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\x01"));
    redraw(&app, AgentKind::Claude, "[Pasted text #3] from bus?");
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\x0b"));
    redraw(&app, AgentKind::Claude, "");
    let response = response.recv_timeout(WAIT).unwrap();
    assert!(response.contains("agent_prompt_not_shown"), "{response}");
    assert!(!response.contains("agent_prompted"));
    while let Ok(bytes) = writes.try_recv() {
        assert_ne!(
            bytes,
            Bytes::from_static(b"\r"),
            "never Enter an unrecognised draft"
        );
    }
}
