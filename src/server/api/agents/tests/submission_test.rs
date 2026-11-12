use super::*;
use crate::protocol::api::schema::{
    AgentPromptIfIdleParams, AgentPromptIfUnboundParams, Method, Request,
};

pub(super) const WAIT: Duration = crate::terminal::runtime::NATIVE_TEST_TIMEOUT;

#[path = "kimi_delivery_test.rs"]
mod kimi_delivery_tests;

#[path = "claude_image_submission_test.rs"]
mod claude_image_submission_tests;

#[path = "claude_plain_submission_test.rs"]
mod claude_plain_submission_tests;

fn clock(app: &App) -> crate::terminal::runtime::SubmissionClock {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_submission_clock()
        .unwrap()
}

fn confirmation_expiry(method: &Method) -> Duration {
    use crate::protocol::api::submission::{method_budget, withdrawal_budget};
    // Provider paste delay varies on ConPTY. Keep the withdrawal reserve
    // while advancing beyond the declared confirmation window.
    let text = match method {
        Method::AgentPromptIfIdle(params) | Method::AgentPromptIfIdleAtGeneration(params) => {
            params.text.as_bytes()
        }
        Method::AgentPromptIfUnbound(params) | Method::AgentPromptIfUnboundAtGeneration(params) => {
            params.text.as_bytes()
        }
        _ => panic!("expected guarded prompt"),
    };
    method_budget(method).unwrap() - withdrawal_budget(text) + Duration::from_millis(100)
}

pub(super) fn provider_fixture(
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
            expected_generation: None,
            expected_turn_id: None,
        })
    } else {
        Method::AgentPromptIfUnbound(AgentPromptIfUnboundParams {
            target: info.pane_id.clone(),
            text: "from bus".into(),
            expected_terminal_id: info.terminal_id,
            expected_pane_id: info.pane_id,
            expected_managed_name: "bus-r1-a2".into(),
            expected_generation: None,
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
async fn generation_fence_rejects_same_terminal_bound_unbound_and_steering_before_input() {
    use crate::server::api::agents::prompt::{
        check_prompt_identity_and_idle, check_unbound_prompt_identity_and_idle,
    };
    for mode in ["bound", "unbound", "steering"] {
        let (mut app, method, mut writes) =
            provider_fixture(AgentKind::Codex, mode != "unbound", "");
        app.state.generation = 1;
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let before = app.agent_info(0, pane).unwrap();
        let method = match method {
            Method::AgentPromptIfIdle(mut params) => {
                params.expected_generation = Some(1);
                params.steer = mode == "steering";
                assert!(
                    check_prompt_identity_and_idle(&before, &params).is_ok(),
                    "{mode}"
                );
                Method::AgentPromptIfIdleAtGeneration(params)
            }
            Method::AgentPromptIfUnbound(mut params) => {
                params.expected_generation = Some(1);
                assert!(check_unbound_prompt_identity_and_idle(&before, &params).is_ok());
                Method::AgentPromptIfUnboundAtGeneration(params)
            }
            _ => unreachable!(),
        };
        app.state.generation = 2;
        let after = app.agent_info(0, pane).unwrap();
        assert_eq!(
            (before.terminal_id, before.pane_id),
            (after.terminal_id, after.pane_id)
        );
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_agent_api_request(
            Request {
                id: mode.into(),
                method
            },
            tx
        ));
        let response: serde_json::Value =
            serde_json::from_str(&rx.recv_timeout(WAIT).unwrap()).unwrap();
        assert_eq!(
            response["error"]["code"], "agent_identity_changed",
            "{mode}: {response}"
        );
        assert!(
            writes.try_recv().is_err(),
            "{mode}: stale generation typed native input"
        );
    }
}

#[tokio::test]
async fn an_expired_dispatch_budget_is_definitely_rejected_before_any_input() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let (respond_to, response) = std::sync::mpsc::channel();
    // Inject queue/drain latency without sleeping: 4 s has consumed the
    // native budget, but the coordinator's 4.3 s read is still open.
    assert!(
        app.handle_deferred_agent_api_message(crate::server::api::ApiRequestMessage {
            received_at: std::time::Instant::now()
                - crate::protocol::api::submission::method_budget(&method).unwrap()
                - Duration::from_secs(1),
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
                    crate::terminal::runtime::PromptNotShown::NotSubmitted,
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
    let paste_receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        response.try_recv().is_err(),
        "a held write cannot complete during descheduling"
    );
    assert!(
        writes.try_recv().is_err(),
        "no Enter before the paste receipt"
    );
    paste_receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    redraw(&app, AgentKind::Codex, "");
    let response = response.recv_timeout(WAIT).unwrap();
    assert!(response.contains("agent_prompted"), "{response}");
    assert!(writes.try_recv().is_err(), "no second paste or Enter");
}

#[tokio::test]
async fn one_flushed_paste_is_submitted_once_without_a_render_readback() {
    for (kind, bound) in [(AgentKind::Cursor, true)] {
        let (mut app, method, mut writes) = provider_fixture(kind, bound, "");
        let receipt = clock(&app).hold_next_write_completion();
        let response = start(&mut app, method);
        assert_eq!(
            next_write(&mut writes).await,
            Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
        );
        assert!(
            writes.try_recv().is_err(),
            "{kind:?}: paste receipt is still pending"
        );
        receipt.send(()).unwrap();
        assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
        let response = response.recv_timeout(WAIT).unwrap();
        assert!(response.contains("agent_prompted"), "{kind:?}: {response}");
        assert!(
            writes.try_recv().is_err(),
            "{kind:?}: no duplicate paste or Enter"
        );
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

#[tokio::test]
async fn correlated_codex_images_use_dedicated_pastes_and_one_enter() {
    for bound in [false, true] {
        let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Codex, bound, "");
        let text = "worker:\n[Bus message 17; answer with bus send --reply-to 17 --as 3 --text 'YOUR ANSWER']\nreview\n\nall pixels";
        let images = ["\"/tmp/a.png\"", "\"/tmp/b.PNG\""];
        let payload = format!("{text}\n{}", images.join("\n"));
        match &mut method {
            Method::AgentPromptIfIdle(params) => params.text = payload,
            Method::AgentPromptIfUnbound(params) => params.text = payload,
            _ => unreachable!(),
        }
        let receipt = clock(&app).hold_next_write_completion();
        let response = start(&mut app, method);
        let expected = format!(
            "\x1b[200~{}\x1b[201~\x1b[200~{}\x1b[201~\x1b[200~{text}\x1b[201~",
            images[0], images[1]
        );
        assert_eq!(next_write(&mut writes).await, Bytes::from(expected));
        assert!(
            writes.try_recv().is_err(),
            "Enter must await the complete image-and-text write receipt"
        );
        receipt.send(()).unwrap();
        redraw(
            &app,
            AgentKind::Codex,
            &format!("[Image #1] [Image #2]{text}"),
        );
        assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
        redraw(&app, AgentKind::Codex, "");
        let response = response.recv_timeout(WAIT).unwrap();
        assert!(response.contains("agent_prompted"), "{response}");
        assert!(
            writes.try_recv().is_err(),
            "one observed submission must not paste or Enter twice"
        );
    }
}

#[tokio::test]
async fn a_delayed_paste_receipt_precedes_the_only_enter() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, false, "");
    let receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    clock(&app).advance(Duration::from_millis(450));
    assert!(
        writes.try_recv().is_err(),
        "Enter must wait for the paste receipt"
    );
    assert!(
        response.try_recv().is_err(),
        "held paste is not a completed submission"
    );
    receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    redraw(&app, AgentKind::Codex, "");
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
    assert!(writes.try_recv().is_err(), "one paste and one Enter only");
}

async fn fail_enter_receipt(
    app: &mut App,
    method: Method,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    kind: AgentKind,
) -> String {
    let clock = clock(app);
    let paste_receipt = clock.hold_next_write_completion();
    let response = start(app, method);
    assert_eq!(
        next_write(writes).await,
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    redraw(app, kind, "from bus");
    let enter_receipt = clock.fail_next_write_completion(std::io::ErrorKind::TimedOut);
    paste_receipt.send(()).unwrap();
    assert_eq!(next_write(writes).await, Bytes::from_static(b"\r"));
    enter_receipt.send(()).unwrap();
    let reply = response.recv_timeout(WAIT).unwrap();
    assert!(reply.contains("timeout"), "{reply}");
    assert!(!reply.contains("agent_prompted"), "{reply}");
    assert!(
        !reply.contains("agent_prompt_not_shown"),
        "lost Enter receipt is uncertain: {reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "no automatic cleanup or old replay"
    );
    reply
}

fn recovery_without_new_input(
    app: &App,
    deadline: Option<std::time::Instant>,
) -> crate::terminal::pty::actor::InputCompletion {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .queue_observed_input_submission(
            Bytes::from_static(b"new message"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            deadline,
            |_| crate::terminal::runtime::InputObservation::Unavailable,
        )
        .unwrap()
}

#[tokio::test]
async fn a_changed_draft_after_a_lost_enter_receipt_is_never_replayed_or_edited() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    fail_enter_receipt(&mut app, method, &mut writes, AgentKind::Codex).await;
    redraw(&app, AgentKind::Codex, "frombus");
    let deadline = clock(&app).now() + Duration::from_millis(100);
    let completion = recovery_without_new_input(&app, Some(deadline));
    clock(&app).advance(Duration::from_millis(200));
    let error = completion.recv_timeout(WAIT).unwrap().unwrap_err();
    assert!(
        crate::terminal::runtime::PromptNotShown::composer_busy(&error),
        "{error}"
    );
    assert!(
        writes.try_recv().is_err(),
        "no Enter, re-paste or destructive edit of foreign input"
    );
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    assert!(app
        .lookup_runtime_sender(0, pane)
        .unwrap()
        .visible_ansi()
        .contains("frombus"));
}

#[tokio::test]
async fn a_cursor_lost_enter_receipt_is_uncertain_and_recovered_without_replay() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Cursor, true, "");
    fail_enter_receipt(&mut app, method, &mut writes, AgentKind::Cursor).await;
    let completion = recovery_without_new_input(&app, None);
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\x01"));
    redraw(&app, AgentKind::Cursor, "from bus");
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\x0b"));
    redraw(&app, AgentKind::Cursor, "");
    let error = completion.recv_timeout(WAIT).unwrap().unwrap_err();
    assert!(
        crate::terminal::runtime::PromptNotShown::is(&error),
        "fresh input remains unavailable: {error}"
    );
    assert!(writes.try_recv().is_err(), "no duplicate Enter or paste");
}

#[tokio::test]
async fn an_unrecognised_paste_is_preserved_without_being_retried() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let expiry = confirmation_expiry(&method);
    let receipt = clock(&app).fail_next_write_completion(std::io::ErrorKind::TimedOut);
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    redraw(&app, AgentKind::Claude, "[Pasted text #3] from bus?");
    clock(&app).advance(expiry);
    receipt.send(()).unwrap();
    let reply = response.recv_timeout(WAIT).unwrap();
    assert!(reply.contains("draft was left"), "{reply}");
    assert!(
        !reply.contains("agent_prompt_not_shown"),
        "unowned input is not safely replayable: {reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "no editing, Enter or second paste"
    );
}

#[path = "paste_recovery_test.rs"]
mod paste_recovery_tests;

#[path = "owned_draft_cleanup_test.rs"]
mod owned_draft_cleanup_tests;

// Before this attempt pastes, even an exact text match does not prove ownership.
pub(super) async fn preserve_existing_draft(kind: AgentKind, bound: bool) {
    for draft in [
        "[Image #138] developer draft",
        "[Pasted text #147 +20 lines]",
    ] {
        let (mut app, method, mut writes) = provider_fixture(kind, bound, draft);
        if kind == AgentKind::Claude {
            let pane = app.state.workspaces[0].tabs[0].root_pane;
            app.lookup_runtime_sender(0, pane)
                .unwrap()
                .test_process_pty_bytes("\r\n› stashed".as_bytes());
        }
        let response = start(&mut app, method);
        let mut response =
            tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
        let (reply, key) = tokio::select! {
            result=&mut response=>(result.unwrap(),None),
            key=writes.recv()=>{
                clock(&app).advance(Duration::from_secs(20));
                (response.await.unwrap(),key)
            }
        };
        assert_eq!(key, None, "pre-paste cleanup touched unowned input");
        assert!(reply.contains("agent_composer_busy"), "{reply}");
        assert!(!reply.contains("agent_prompt_not_shown"), "{reply}");
        assert!(
            writes.try_recv().is_err(),
            "no clear key, new paste or Enter"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn legacy_api_timeout_cancels_committed_input_before_backpressure_lifts() {
    use crate::terminal::pty::actor::{PtyIoActor, PtyIoActorConfig, PtyReadResult};
    use std::{
        io::Read,
        os::{fd::AsRawFd, unix::net::UnixStream},
        time::Instant,
    };

    let (app, _, _) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let agent = app.agent_info(0, pane).unwrap();
    let (socket, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let actor = PtyIoActor::spawn(PtyIoActorConfig {
        pane_id: 1,
        master_fd: socket.into(),
        initially_quiesced: false,
        on_read: Box::new(|_| PtyReadResult::empty()),
    })
    .unwrap();
    let text_len = 4 * 1024 * 1024;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(1);
    let completion = actor
        .queue_user_input_submission(
            Bytes::from(vec![b'x'; text_len]),
            Bytes::from_static(b"\r"),
            Duration::from_millis(50),
            Some(deadline),
        )
        .unwrap()
        .into_receiver();
    // Commit the real actor's first text byte, then stop draining its socket.
    let mut first = [0; 1];
    peer.read_exact(&mut first).unwrap();
    assert_eq!(first, [b'x']);
    let response = crate::server::api::agents::prompt::complete_agent_prompt(
        "backpressured_legacy".into(),
        agent,
        deadline,
        started,
        Instant::now(),
        move |timeout| completion.recv_timeout(timeout),
    );
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["error"]["code"], "timeout");

    // Separate bytes already in the kernel at the response boundary from any
    // later actor writes: the pre-response prefix itself cannot be retracted.
    let mut buffered: libc::c_int = 0;
    assert_eq!(
        unsafe { libc::ioctl(peer.as_raw_fd(), libc::FIONREAD, &mut buffered) },
        0
    );
    assert!(buffered > 0 && (buffered as usize) < text_len - 1);
    actor
        .try_write_user_input(Bytes::from_static(b"after"))
        .unwrap();
    let mut prefix = vec![0; buffered as usize];
    peer.read_exact(&mut prefix).unwrap();
    assert!(prefix.iter().all(|byte| *byte == b'x'));
    let mut after_response = Vec::new();
    let read_result = loop {
        if after_response.ends_with(b"after") {
            break Ok(());
        }
        let mut chunk = [0; 8192];
        match peer.read(&mut chunk) {
            Ok(0) => break Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
            Ok(n) => after_response.extend_from_slice(&chunk[..n]),
            Err(error) => break Err(error),
        }
        if after_response.len() > text_len + 6 {
            break Err(std::io::Error::other("unexpected actor output"));
        }
    };
    actor.shutdown();
    read_result.expect("actor must resume and flush the following input marker");
    let late_text = after_response.iter().filter(|byte| **byte == b'x').count();
    let late_enters = after_response.iter().filter(|byte| **byte == b'\r').count();
    assert_eq!(
        (late_text, late_enters),
        (0, 0),
        "submission bytes reached the PTY after API timeout; response={response}"
    );
    assert_eq!(after_response, b"after");
    let guidance = response["error"]["message"].as_str().unwrap();
    assert!(
        guidance.contains("partial")
            && guidance.contains("unsubmitted")
            && guidance.contains("draft")
            && guidance.contains("before resending"),
        "partial unsubmitted draft needs actionable guidance: {guidance}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn api_waits_for_committed_enter_within_its_delay_allowance() {
    use crate::terminal::pty::actor::{PtyIoActor, PtyIoActorConfig, PtyReadResult};
    use std::{io::Read, os::unix::net::UnixStream, time::Instant};
    let (app, _, _) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let agent = app.agent_info(0, pane).unwrap();
    let (socket, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let actor = PtyIoActor::spawn(PtyIoActorConfig {
        pane_id: 1,
        master_fd: socket.into(),
        initially_quiesced: false,
        on_read: Box::new(|_| PtyReadResult::empty()),
    })
    .unwrap();
    let text_len = 4 * 1024 * 1024;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(1);
    // Exercise a nonzero allowance independently of the provider's default
    // pacing, including providers whose normal submit delay is zero.
    let delay = Duration::from_millis(300);
    let completion = actor
        .queue_user_input_submission(
            Bytes::from(vec![b'x'; text_len]),
            Bytes::from_static(b"\r"),
            delay,
            // The API timeline below is virtual. Actor deadline enforcement
            // is covered in input_deadline_test; host scheduling must not
            // consume this fixture's synthetic response budget.
            None,
        )
        .unwrap();
    let mut first = [0; 1];
    peer.read_exact(&mut first).unwrap();
    assert_eq!(first, [b'x']);
    actor
        .try_write_user_input(Bytes::from_static(b"after"))
        .unwrap();
    let mut received = vec![0; text_len + 1 + 5];
    received[0] = first[0];
    let response = crate::server::api::agents::prompt::complete_agent_prompt_submission(
        "committed_enter".into(),
        agent,
        deadline,
        started,
        deadline + Duration::from_millis(150),
        completion,
        |timeout| {
            // 150 ms of the 300 ms Enter allowance remains, plus the
            // existing 100 ms response grace. Without the allowance this
            // late API waiter would immediately cancel the committed input.
            assert_eq!(timeout, Duration::from_millis(250));
            // Release real backpressure only once the API is waiting. Reading
            // the following marker proves the actor published its receipt;
            // no scheduler race remains between that receipt and recv_timeout.
            peer.read_exact(&mut received[1..]).unwrap();
            timeout
        },
    );
    actor.shutdown();
    assert!(
        response.contains("agent_prompted"),
        "normal committed delivery timed out: {response}"
    );
    let body: serde_json::Value = serde_json::from_str(&response).unwrap();
    let timestamps = &body["result"]["timestamps"];
    let text_written = timestamps["text_written_at_ms"].as_u64().unwrap();
    let enter_written = timestamps["enter_written_at_ms"].as_u64().unwrap();
    assert!(text_written <= enter_written);
    assert!(received[..text_len].iter().all(|byte| *byte == b'x'));
    assert_eq!(&received[text_len..], b"\rafter");
}

#[cfg(unix)]
#[tokio::test]
async fn api_timeout_revokes_future_observed_scope_keys() {
    use crate::terminal::pty::actor::{PtyIoActor, PtyIoActorConfig, PtyReadResult};
    use std::{io::Read, os::unix::net::UnixStream, time::Instant};
    let (app, _, _) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let agent = app.agent_info(0, pane).unwrap();
    let (socket, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let actor = PtyIoActor::spawn(PtyIoActorConfig {
        pane_id: 1,
        master_fd: socket.into(),
        initially_quiesced: false,
        on_read: Box::new(|_| PtyReadResult::empty()),
    })
    .unwrap();
    let scope = actor.input_scope();
    let paste = actor
        .queue_user_input_submission_scoped(
            Bytes::from_static(b"owned"),
            Bytes::new(),
            Duration::ZERO,
            None,
            &scope,
            true,
        )
        .unwrap();
    let mut draft = [0; 5];
    peer.read_exact(&mut draft).unwrap();
    assert_eq!(&draft, b"owned");
    paste.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
    // Model an observer still waiting to finish after its successful paste.
    let (_pending, pending_rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    let response = crate::server::api::agents::prompt::complete_agent_prompt_submission(
        "observed_scope".into(),
        agent,
        started,
        started,
        started,
        scope.completion(pending_rx),
        std::convert::identity,
    );
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["error"]["code"], "timeout");
    let late_key = actor
        .queue_user_input_submission_scoped(
            Bytes::from_static(b"\x01"),
            Bytes::new(),
            Duration::ZERO,
            None,
            &scope,
            false,
        )
        .unwrap();
    let error = late_key
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    actor
        .try_write_user_input(Bytes::from_static(b"after"))
        .unwrap();
    let mut following = [0; 5];
    let read = peer.read_exact(&mut following);
    actor.shutdown();
    read.unwrap();
    assert_eq!(
        &following, b"after",
        "an observed key escaped after API timeout"
    );
    let guidance = response["error"]["message"].as_str().unwrap();
    assert!(
        guidance.contains("draft") && guidance.contains("before resending"),
        "{guidance}"
    );
}

async fn claude_empty_composer_receives_the_prompt_below(above: &str) {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(
            format!(
                "\x1b[2J\x1b[H{above}\r\n────────────────────────\r\n❯ \r\n────────────────────────"
            )
            .as_bytes(),
        );
    let response = start(&mut app, method);
    let mut response = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
    tokio::select! {
        reply = &mut response => panic!("empty composer was not typed into: {}", reply.unwrap()),
        paste = next_write(&mut writes) => {
            assert_eq!(paste, Bytes::from_static(b"\x1b[200~from bus\x1b[201~"));
        }
    }
    redraw(&app, AgentKind::Claude, "from bus");
    if above.contains("› stashed") {
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_process_pty_bytes("\r\n› stashed".as_bytes());
    }
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    redraw(
        &app,
        AgentKind::Claude,
        if above.contains("› stashed") {
            "developer saved draft"
        } else {
            ""
        },
    );
    assert!(response.await.unwrap().contains("agent_prompted"));
    assert!(
        writes.try_recv().is_err(),
        "restored draft must not receive another Enter"
    );
}

#[tokio::test]
async fn claude_empty_composer_below_a_transcript_mentioning_stashed_receives_the_prompt() {
    claude_empty_composer_receives_the_prompt_below(
        "⏺ I stashed the local changes before rebasing.",
    )
    .await;
}

#[tokio::test]
async fn claude_empty_composer_with_a_developer_stash_receives_the_prompt() {
    claude_empty_composer_receives_the_prompt_below(
        "                    ◐ medium · /effort · › stashed",
    )
    .await;
}

#[cfg(unix)]
#[path = "codex_native_test.rs"]
mod codex_native_tests;

#[path = "codex_legacy_submission_test.rs"]
mod codex_legacy_submission_tests;
