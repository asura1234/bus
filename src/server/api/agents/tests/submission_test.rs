use super::*;
use crate::protocol::api::schema::{
    AgentPromptIfIdleParams, AgentPromptIfUnboundParams, Method, Request,
};

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
async fn guarded_submission_rejects_unavailable_composer_before_any_write() {
    let (app, _, mut writes) = provider_fixture(AgentKind::Codex, false, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
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
    let error = completion
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap_err();
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
    let error = completion
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap_err();
    assert!(writes.try_recv().is_err(), "startup control received input");
    assert!(
        crate::terminal::runtime::PromptNotShown::is(&error),
        "a refusal before any write must retain the queued message: {error}"
    );
}

pub(super) fn start(app: &mut App, method: Method) -> std::sync::mpsc::Receiver<String> {
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

pub(super) fn next_write(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(bytes) = writes.try_recv() {
            return bytes;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected native input write"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
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
            next_write(&mut writes),
            Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
        );
        redraw(&app, kind, "from bus");
        assert_eq!(next_write(&mut writes), Bytes::from_static(b"\r"));
        std::thread::sleep(Duration::from_millis(40));
        assert!(
            response.try_recv().is_err(),
            "{kind:?}: PTY write is not provider acceptance"
        );
        assert_eq!(next_write(&mut writes), Bytes::from_static(b"\r"));
        redraw(&app, kind, "");
        let response = response.recv_timeout(Duration::from_secs(2)).unwrap();
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
        stranded_draft(kind, bound);
    }
}

pub(super) fn accept_paste(
    app: &App,
    kind: AgentKind,
    text: &str,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
) {
    assert_eq!(
        next_write(writes),
        Bytes::from(format!("\x1b[200~{text}\x1b[201~"))
    );
    redraw(app, kind, text);
    assert_eq!(next_write(writes), Bytes::from_static(b"\r"));
    redraw(app, kind, "");
}

pub(super) fn stranded_draft(kind: AgentKind, bound: bool) {
    let (mut app, method, mut writes) = provider_fixture(kind, bound, "stranded draft");
    let response = start(&mut app, method);
    // Ink-based providers can decode a burst of control characters as one
    // event. Send each editing key separately and allow a redraw between them.
    assert_eq!(next_write(&mut writes), Bytes::from_static(b"\x01"));
    assert_eq!(next_write(&mut writes), Bytes::from_static(b"\x0b"));
    redraw(&app, kind, "");
    accept_paste(&app, kind, "from bus", &mut writes);
    let response = response.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(response.contains("agent_prompted"), "{kind:?}: {response}");
}

#[tokio::test]
async fn delayed_paste_render_waits_before_the_first_enter() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, false, "");
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes),
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    std::thread::sleep(Duration::from_millis(450));
    assert!(
        writes.try_recv().is_err(),
        "Enter must wait until provider consumed paste"
    );
    redraw(&app, AgentKind::Codex, "from bus");
    assert_eq!(next_write(&mut writes), Bytes::from_static(b"\r"));
    redraw(&app, AgentKind::Codex, "");
    assert!(response
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .contains("agent_prompted"));
}

#[tokio::test]
async fn changed_draft_after_enter_is_never_submitted_by_retry() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes),
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    redraw(&app, AgentKind::Codex, "from bus");
    assert_eq!(next_write(&mut writes), Bytes::from_static(b"\r"));
    redraw(&app, AgentKind::Codex, "frombus");
    let response = response.recv_timeout(Duration::from_secs(3)).unwrap();
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
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes),
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    redraw(&app, AgentKind::Cursor, "from bus");
    let response = response.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(response.contains("timeout"), "{response}");
    assert!(!response.contains("agent_prompted"));
    let mut enters = 0;
    while let Ok(bytes) = writes.try_recv() {
        assert_eq!(bytes, Bytes::from_static(b"\r"));
        enters += 1;
    }
    assert!(enters > 1, "must retry ignored Enter");
}

#[tokio::test]
async fn an_unrecognised_paste_is_cleared_and_reported_as_not_submitted() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes),
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    // A rendering the observer cannot attribute to Bus: never press Enter on it.
    redraw(&app, AgentKind::Claude, "[Pasted text #3] from bus?");
    std::thread::sleep(Duration::from_millis(2_200));
    assert_eq!(next_write(&mut writes), Bytes::from_static(b"\x01"));
    assert_eq!(next_write(&mut writes), Bytes::from_static(b"\x0b"));
    redraw(&app, AgentKind::Claude, "");
    let response = response.recv_timeout(Duration::from_secs(3)).unwrap();
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
