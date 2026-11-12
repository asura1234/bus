//! Request 5251: a flushed plain paste and Enter are not provider acceptance.
use super::*;

#[tokio::test]
async fn claude_plain_paste_waits_for_the_owned_composer_under_load() {
    for steer in [false, true] {
        let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
        let payload = format!(
            "[Bus message 5250 from peer]\n{}\nScreenshot：/tmp/report.png",
            "Controlled report detail. ".repeat(55)
        );
        match &mut method {
            Method::AgentPromptIfIdle(params) => {
                params.text = payload.clone();
                params.steer = steer;
            }
            _ => unreachable!(),
        }
        let response = start(&mut app, method);
        let paste = tokio::time::timeout(WAIT, writes.recv())
            .await
            .unwrap_or_else(|error| {
                panic!("plain paste: {error}; response: {:?}", response.try_recv())
            })
            .unwrap();
        assert_eq!(paste, Bytes::from(format!("\x1b[200~{payload}\x1b[201~")));
        let waited = clock(&app).wait_for_draft_wait_or_enter();
        if waited {
            // The fake provider drops Enter until its delayed paste is ready.
            clock(&app).advance(Duration::from_millis(400));
            redraw(&app, AgentKind::Claude, "[Pasted text #1 +2 lines]");
        }
        assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
        redraw(&app, AgentKind::Claude, "");
        assert!(response
            .recv_timeout(WAIT)
            .unwrap()
            .contains("agent_prompted"));
        assert!(writes.try_recv().is_err(), "paste is never repeated");
        assert!(
            waited,
            "request 5251: Enter overtook the delayed plain paste"
        );
    }
}

#[tokio::test]
async fn claude_plain_paste_represses_enter_for_the_same_unconsumed_draft() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await.as_ref(),
        b"\x1b[200~from bus\x1b[201~"
    );
    redraw(&app, AgentKind::Claude, "from bus");
    receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    clock(&app).wait_for_enters(1);
    clock(&app).advance(Duration::from_millis(300));
    let response = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
    tokio::pin!(response);
    tokio::select! {
        reply = &mut response => panic!("request 5251: API completed before the owned-draft retry: {}", reply.unwrap()),
        key = next_write(&mut writes) => assert_eq!(key.as_ref(), b"\r"),
    }
    clock(&app).wait_for_enters(2);
    let generation = clock(&app).advance(Duration::from_millis(300));
    clock(&app).wait_for_submission_check(generation);
    assert!(
        writes.try_recv().is_err(),
        "only one extra Enter is allowed"
    );
    redraw(&app, AgentKind::Claude, "");
    assert!(response.await.unwrap().contains("agent_prompted"));
    assert!(writes.try_recv().is_err(), "one paste, at most two Enters");
}

#[tokio::test]
async fn claude_plain_paste_never_retries_a_changed_or_blocked_composer() {
    for body in [
        "from bus edited",
        "Do you trust this folder?\n❯ 1. Yes\n  2. No",
    ] {
        let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
        let expiry = confirmation_expiry(&method);
        let receipt = clock(&app).hold_next_write_completion();
        let response = start(&mut app, method);
        next_write(&mut writes).await;
        redraw(&app, AgentKind::Claude, "from bus");
        receipt.send(()).unwrap();
        assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
        clock(&app).wait_for_enters(1);
        redraw(&app, AgentKind::Claude, body);
        let generation = clock(&app).advance(Duration::from_millis(300));
        clock(&app).wait_for_submission_check(generation);
        assert!(
            writes.try_recv().is_err(),
            "changed or blocked input received Enter"
        );
        clock(&app).advance(expiry);
        let reply = response.recv_timeout(WAIT).unwrap();
        assert!(!reply.contains("agent_prompted"), "{reply}");
        assert!(
            writes.try_recv().is_err(),
            "changed input must remain intact"
        );
    }
}

#[tokio::test]
async fn claude_plain_paste_buffers_developer_keys_without_retrying_enter() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, method);
    next_write(&mut writes).await;
    redraw(&app, AgentKind::Claude, "from bus");
    receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    clock(&app).wait_for_enters(1);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .try_send_developer_bytes(Bytes::from_static(b"developer input"))
        .unwrap();
    let generation = clock(&app).advance(Duration::from_millis(300));
    clock(&app).wait_for_submission_check(generation);
    assert!(
        writes.try_recv().is_err(),
        "developer input revokes the retry"
    );
    redraw(&app, AgentKind::Claude, "");
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
    assert_eq!(next_write(&mut writes).await.as_ref(), b"developer input");
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn claude_plain_paste_lost_enter_receipt_is_never_retried() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    fail_enter_receipt(&mut app, method, &mut writes, AgentKind::Claude).await;
}

#[tokio::test]
async fn claude_plain_paste_does_not_confirm_unrelated_background_activity() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, method);
    next_write(&mut writes).await;
    redraw(&app, AgentKind::Claude, "from bus");
    receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    clock(&app).wait_for_enters(1);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes("\x1b[2J\x1b[H✻ Thinking…\r\n".as_bytes());
    let generation = clock(&app).advance(Duration::from_millis(300));
    clock(&app).wait_for_submission_check(generation);
    assert!(
        response.try_recv().is_err(),
        "foreign activity is not consumption"
    );
    assert!(
        writes.try_recv().is_err(),
        "no Enter without the owned composer"
    );
    redraw(&app, AgentKind::Claude, "from bus");
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    redraw(&app, AgentKind::Claude, "");
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
    assert!(writes.try_recv().is_err());
}
