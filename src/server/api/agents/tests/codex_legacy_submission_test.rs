//! A PTY receipt does not mean Codex consumed the prompt.
use super::owned_draft_cleanup_tests::write_or_done;
use super::*;

async fn ignored_enter(developer_edit: bool) {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let expiry = confirmation_expiry(&method);
    let native_clock = clock(&app);
    let paste_receipt = native_clock.hold_next_write_completion();
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await.as_ref(),
        b"\x1b[200~from bus\x1b[201~"
    );
    redraw(&app, AgentKind::Codex, "from bus");
    let enter_receipt = native_clock.hold_next_write_completion();
    paste_receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    // Codex's picker can consume Enter while leaving the composer intact.
    // The writer acknowledges it, but no model turn has accepted this text.
    enter_receipt.send(()).unwrap();
    native_clock.wait_for_enters(1);
    if developer_edit {
        redraw(&app, AgentKind::Codex, "from bus + developer draft");
    }
    native_clock.advance(expiry);
    let mut response = (
        tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap()),
        None,
    );
    let mut clears = 0;
    let reply = loop {
        match write_or_done(&mut writes, &mut response).await {
            Ok(key) => {
                assert!(
                    !developer_edit,
                    "cleanup edited the developer's draft: {key:?}"
                );
                assert!(
                    matches!(key.as_ref(), b"\x01" | b"\x0b" | b"\x15"),
                    "never replay Enter or paste: {key:?}"
                );
                clears += 1;
                if key.as_ref() == b"\x0b" {
                    redraw(&app, AgentKind::Codex, "");
                } else {
                    redraw(&app, AgentKind::Codex, "from bus");
                }
            }
            Err(reply) => break reply,
        }
    };
    assert!(
        !reply.contains("agent_prompted"),
        "a flushed Enter is not consumption: {reply}"
    );
    assert!(
        !reply.contains("agent_prompt_not_shown"),
        "Enter may already have run: {reply}"
    );
    if developer_edit {
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        assert!(app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .visible_ansi()
            .contains("from bus + developer draft"));
        assert_eq!(clears, 0);
    } else {
        assert!(clears > 0, "withdraw the exact stranded Bus text");
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        assert!(!app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .visible_ansi()
            .contains("from bus"));
    }
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn codex_legacy_ignored_enter_withdraws_owned_text() {
    ignored_enter(false).await;
}

#[tokio::test]
async fn codex_legacy_ignored_enter_preserves_developer_edit() {
    ignored_enter(true).await;
}

#[test]
fn codex_legacy_pickers_do_not_accept_delivery_keys() {
    for screen in [
        "› \n\n GPT-6 · /repo\n reverse-i-search: no match",
        "Mentions\nFiles, directories, plugins, skills and tasks\nno matches\nenter/tab insert · esc close · ↑/↓ select · ←/→ filter\n\n› developer draft @file\n\n GPT-6 · /repo",
    ] {
        let inspector = crate::server::api::agents::prompt::detection_composer(AgentKind::Codex).unwrap();
        assert!(!inspector.submission_available(screen), "picker intercepts input: {screen}");
        assert!(!inspector.draft_ready(screen, None, false));
    }
}

#[test]
fn codex_legacy_picker_words_in_a_draft_are_literal_text() {
    let inspector =
        crate::server::api::agents::prompt::detection_composer(AgentKind::Codex).unwrap();
    assert!(inspector.submission_available(
        "› Explain reverse-i-search: and Mentions\n  enter/tab insert · esc close · ↑/↓ select\n\n GPT-6 · /repo\n ? for shortcuts"
    ));
}

#[tokio::test]
async fn codex_legacy_late_render_after_empty_frame_retains_owned_cleanup() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let paste_receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await.as_ref(),
        b"\x1b[200~from bus\x1b[201~"
    );
    redraw(&app, AgentKind::Codex, "from bus");
    paste_receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    // A complete empty repaint is only composer evidence, not a native claim.
    redraw(&app, AgentKind::Codex, "");
    let reply = response.recv_timeout(WAIT).unwrap();
    assert!(reply.contains("agent_prompted"), "{reply}");
    redraw(&app, AgentKind::Codex, "from bus");
    let completion = recovery_without_new_input(&app, None);
    let mut response = (
        tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap()),
        None,
    );
    let mut cleared = false;
    let error = loop {
        match write_or_done(&mut writes, &mut response).await {
            Ok(key) => {
                assert!(
                    matches!(key.as_ref(), b"\x01" | b"\x0b" | b"\x15"),
                    "no old Enter or new paste: {key:?}"
                );
                if key.as_ref() == b"\x0b" {
                    cleared = true;
                    redraw(&app, AgentKind::Codex, "");
                } else {
                    redraw(&app, AgentKind::Codex, "from bus");
                }
            }
            Err(result) => break result.unwrap_err(),
        }
    };
    assert!(
        cleared,
        "a transient empty frame lost the old Bus ownership record: {error}"
    );
    assert!(crate::terminal::runtime::PromptNotShown::is(&error));
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn codex_legacy_picker_after_paste_is_closed_only_for_owned_withdrawal() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let paste_receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, method);
    assert_eq!(
        next_write(&mut writes).await.as_ref(),
        b"\x1b[200~from bus\x1b[201~"
    );
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane).unwrap().test_process_pty_bytes(
        "\x1b[2J\x1b[HMentions\r\nFiles, directories, plugins, skills and tasks\r\nno matches\r\nenter/tab insert · esc close · ↑/↓ select · ←/→ filter\r\n\r\n› from bus\r\n\r\n GPT-6 · /repo".as_bytes(),
    );
    paste_receipt.send(()).unwrap();
    assert_eq!(
        next_write(&mut writes).await.as_ref(),
        b"\x1b",
        "only dismiss the owned picker; no Enter"
    );
    redraw(&app, AgentKind::Codex, "from bus");
    let mut response = (
        tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap()),
        None,
    );
    let mut clears = 0;
    let reply = loop {
        match write_or_done(&mut writes, &mut response).await {
            Ok(key) => {
                assert!(
                    matches!(key.as_ref(), b"\x01" | b"\x0b" | b"\x15"),
                    "no Enter or repeated Esc: {key:?}"
                );
                clears += 1;
                redraw(
                    &app,
                    AgentKind::Codex,
                    if key.as_ref() == b"\x0b" {
                        ""
                    } else {
                        "from bus"
                    },
                );
            }
            Err(reply) => break reply,
        }
    };
    assert!(
        reply.contains("agent_prompt_not_shown"),
        "no Enter and a proven withdrawal: {reply}"
    );
    assert!(clears > 0);
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn codex_legacy_developer_input_revokes_retained_cleanup_even_for_identical_text() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let response = start(&mut app, method);
    accept_paste(&app, AgentKind::Codex, "from bus", &mut writes).await;
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .try_send_developer_bytes(Bytes::from_static(b"from bus"))
        .unwrap();
    assert_eq!(next_write(&mut writes).await.as_ref(), b"from bus");
    redraw(&app, AgentKind::Codex, "from bus");
    let error = recovery_without_new_input(&app, None)
        .recv_timeout(WAIT)
        .unwrap()
        .unwrap_err();
    assert!(crate::terminal::runtime::PromptNotShown::is(&error));
    assert!(
        writes.try_recv().is_err(),
        "never clear identical text typed by the developer"
    );
    assert!(app
        .lookup_runtime_sender(0, pane)
        .unwrap()
        .visible_ansi()
        .contains("from bus"));
}

#[tokio::test]
async fn codex_legacy_owned_opaque_paste_remains_eligible_for_cleanup() {
    let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let payload = "owned payload ".repeat(80);
    let Method::AgentPromptIfIdle(params) = &mut method else {
        unreachable!()
    };
    params.text = payload.clone();
    let response = start(&mut app, method);
    accept_paste(&app, AgentKind::Codex, &payload, &mut writes).await;
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
    redraw(
        &app,
        AgentKind::Codex,
        &format!("[Pasted Content {} chars]", payload.chars().count()),
    );
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    runtime.set_detection_context(
        AgentKind::Codex,
        true,
        None,
        crate::server::api::agents::prompt::detection_composer(AgentKind::Codex),
    );
    let state = runtime
        .test_scan_detection_context(AgentKind::Codex, AgentState::Unknown)
        .await
        .0;
    assert_eq!(
        state,
        AgentState::Idle,
        "the old Bus-owned chip must reach cleanup instead of blocking the next message"
    );
    assert!(
        writes.try_recv().is_err(),
        "readiness must not edit or submit input"
    );
    runtime
        .try_send_developer_bytes(Bytes::from(payload.clone()))
        .unwrap();
    assert_eq!(next_write(&mut writes).await.as_ref(), payload.as_bytes());
    let state = runtime
        .test_scan_detection_context(AgentKind::Codex, AgentState::Unknown)
        .await
        .0;
    assert_eq!(
        state,
        AgentState::Blocked,
        "developer input revokes the old ownership even when its chip looks identical"
    );
    assert!(writes.try_recv().is_err());
}
