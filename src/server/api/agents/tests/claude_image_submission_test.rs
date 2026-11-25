use super::*;

use crate::server::api::test_support::DraftTestEnvironment;

async fn step(
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    response: &std::sync::mpsc::Receiver<String>,
    phase: &str,
) -> Bytes {
    tokio::time::timeout(WAIT, writes.recv())
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{phase}: {error}; native response: {:?}",
                response.try_recv()
            )
        })
        .unwrap()
}

async fn focused_delivery(draft: &str, image: bool) {
    let _scratch = DraftTestEnvironment::new("claude-image-delivery");
    let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Claude, true, draft);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(pane));
    let text = if image {
        "from bus\n\"/tmp/large.png\""
    } else {
        "from bus"
    };
    if let Method::AgentPromptIfIdle(params) = &mut method {
        params.text = text.into();
    }
    let response = start(&mut app, method);
    if !draft.is_empty() {
        assert_eq!(
            step(&mut writes, &response, "stash").await.as_ref(),
            b"\x13"
        );
        redraw(&app, AgentKind::Claude, "");
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_process_pty_bytes("\r\n› stashed".as_bytes());
    }
    assert_eq!(
        step(&mut writes, &response, "paste").await,
        Bytes::from(format!("\x1b[200~{text}\x1b[201~"))
    );
    if image {
        // Claude 2.1.316 loads image paths asynchronously. Enter during this
        // interval is consumed by its paste handler without submitting.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), writes.recv())
                .await
                .is_err(),
            "Enter must follow image registration, not the PTY write receipt"
        );
        assert!(response.try_recv().is_err());
        redraw(&app, AgentKind::Claude, "[Image #7]from bus");
    } else {
        redraw(&app, AgentKind::Claude, text);
    }
    assert_eq!(step(&mut writes, &response, "submit").await.as_ref(), b"\r");
    // New developer input remains buffered until the native stash restores.
    if !draft.is_empty() {
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .try_send_developer_bytes(Bytes::from_static(b" more"))
            .unwrap();
    }
    redraw(&app, AgentKind::Claude, draft);
    let reply = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap())
        .await
        .unwrap();
    assert!(reply.contains("agent_prompted"), "{reply}");
    if !draft.is_empty() {
        assert_eq!(next_write(&mut writes).await.as_ref(), b" more");
        assert!(app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .visible_text()
            .contains(draft));
    }
    assert!(writes.try_recv().is_err(), "one paste and one submit only");
}

#[tokio::test]
async fn focused_idle_claude_waits_only_for_image_registration_before_submit() {
    focused_delivery("", true).await;
}

#[tokio::test]
async fn focused_idle_claude_submits_image_and_restores_half_typed_draft() {
    focused_delivery("half-typed developer draft", true).await;
}

#[tokio::test]
async fn focused_idle_claude_submits_text_after_the_owned_paste_frame() {
    focused_delivery("", false).await;
}

#[tokio::test]
async fn focused_idle_claude_submits_text_and_restores_half_typed_draft() {
    focused_delivery("half-typed developer draft", false).await;
}

#[tokio::test]
async fn focused_idle_claude_submits_a_shell_escaped_screenshot_only_after_its_chip() {
    let _scratch = DraftTestEnvironment::new("claude-image-delivery");
    let text =
        "/tmp/Screenshot\\ 2026-11-15\\ at\\ 10.27.59\u{202f}PM.png \nkeep the green background";
    let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    if let Method::AgentPromptIfIdle(params) = &mut method {
        params.text = text.into();
    }
    let response = start(&mut app, method);
    assert_eq!(
        step(&mut writes, &response, "paste").await,
        Bytes::from(format!("\x1b[200~{text}\x1b[201~"))
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), writes.recv())
            .await
            .is_err(),
        "Enter must wait until the escaped screenshot is an image chip"
    );
    redraw(
        &app,
        AgentKind::Claude,
        "[Image #9]keep the green background",
    );
    assert_eq!(step(&mut writes, &response, "submit").await.as_ref(), b"\r");
    redraw(&app, AgentKind::Claude, "");
    let reply = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap())
        .await
        .unwrap();
    assert!(reply.contains("agent_prompted"), "{reply}");
}

#[tokio::test]
async fn focused_working_claude_defers_without_touching_the_developer_draft() {
    let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(
            concat!(
                "\x1b[2J\x1b[H✻ Coalescing… (7s · ↓ 301 tokens)\r\n",
                "────────────────────────\r\n",
                "❯ developer draft\r\n",
                "────────────────────────\r\n"
            )
            .as_bytes(),
        );
    if let Method::AgentPromptIfIdle(params) = &mut method {
        params.steer = true;
        params.text = "from bus\n\"/tmp/large.png\"".into();
    }
    let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
    assert!(response.contains("agent_claude_turn_busy"), "{response}");
    assert!(writes.try_recv().is_err(), "must neither stash nor paste");
    assert!(app
        .lookup_runtime_sender(0, pane)
        .unwrap()
        .visible_text()
        .contains("developer draft"));
}

#[tokio::test]
async fn focused_claude_image_waits_for_the_native_queue_to_empty() {
    let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    if let Method::AgentPromptIfIdle(params) = &mut method {
        params.text = "from bus\n\"/tmp/large.png\"".into();
    }
    let response = start(&mut app, method);
    step(&mut writes, &response, "paste").await;
    redraw(&app, AgentKind::Claude, "[Image #7]from bus");
    assert_eq!(step(&mut writes, &response, "submit").await.as_ref(), b"\r");
    clock(&app).wait_for_enters(1);
    redraw(
        &app,
        AgentKind::Claude,
        "\x1b[2mPress up to edit queued messages\x1b[22m",
    );
    let generation = clock(&app).advance(Duration::from_millis(300));
    clock(&app).wait_for_submission_check(generation);
    assert!(response.try_recv().is_err(), "queue is still unconsumed");
    redraw(&app, AgentKind::Claude, "");
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
    assert!(writes.try_recv().is_err(), "no second Enter for an image");
}

#[tokio::test]
async fn claude_unverified_stash_binding_defers_without_writing() {
    let scratch = DraftTestEnvironment::new("claude-image-delivery");
    let config = scratch.root.join("claude");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(config.join("keybindings.json"), "{}").unwrap();
    let kind = AgentKind::Claude;
    for draft in ["developer draft", "   "] {
        let (mut app, method, mut writes) = provider_fixture(kind, true, draft);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        if draft.trim().is_empty() {
            app.lookup_runtime_sender(0, pane)
                .unwrap()
                .test_process_pty_bytes(b"\x1b[2;6H");
        }
        let response = start(&mut app, method).recv_timeout(WAIT).unwrap();
        assert!(
            response.contains("agent_composer_busy"),
            "{draft:?}: {response}"
        );
        assert!(
            writes.try_recv().is_err(),
            "unverified stash binding received input"
        );
    }
}

#[tokio::test]
async fn claude_interrupt_replays_returned_image_only_with_bus_ownership() {
    let _scratch = DraftTestEnvironment::new("claude-image-delivery");
    let kind = AgentKind::Claude;
    let aborted = "old prompt\n\"/tmp/t-25.png\"";
    let combined = format!("{aborted}\n\nnew instruction");
    let returned = "[Image #8]old prompt";
    for developer_edited in [true, false] {
        let (mut app, mut lead, mut writes) = provider_fixture(kind, true, "");
        let mut follow = lead.clone();
        let Method::AgentPromptIfIdle(params) = &mut lead else {
            unreachable!()
        };
        params.text = aborted.into();
        let response = start(&mut app, lead);
        assert_eq!(
            step(&mut writes, &response, "lead paste").await,
            Bytes::from(format!("\x1b[200~{aborted}\x1b[201~"))
        );
        redraw(&app, kind, returned);
        assert_eq!(
            step(&mut writes, &response, "lead enter").await.as_ref(),
            b"\r"
        );
        redraw(&app, kind, "");
        assert!(response
            .recv_timeout(WAIT)
            .unwrap()
            .contains("agent_prompted"));

        let pane = app.state.workspaces[0].tabs[0].root_pane;
        if developer_edited {
            let edit = Bytes::from_static(b"local edit");
            app.lookup_runtime_sender(0, pane)
                .unwrap()
                .try_send_developer_bytes(edit.clone())
                .unwrap();
            assert_eq!(next_write(&mut writes).await, edit);
        }
        let agent = app.agent_info(0, pane).unwrap();
        let stop = Method::AgentSendKeys(crate::protocol::api::schema::AgentSendKeysParams {
            target: agent.pane_id,
            keys: vec!["esc".into()],
            expected_terminal_id: Some(agent.terminal_id),
            expected_activity: None,
            native_interrupt: None,
        });
        for attempt in 0..2 {
            let response = app.handle_api_request(Request {
                id: format!("stop-{attempt}"),
                method: stop.clone(),
            });
            assert!(response.contains("\"type\":\"ok\""), "{response}");
            assert_eq!(next_write(&mut writes).await.as_ref(), b"\x1b");
            redraw(&app, kind, returned);
        }

        let Method::AgentPromptIfIdle(params) = &mut follow else {
            unreachable!()
        };
        params.text = combined.clone();
        params.discard_aborted_payload = Some(aborted.into());
        let response = start(&mut app, follow);
        if developer_edited {
            assert_eq!(
                step(&mut writes, &response, "stash foreign image")
                    .await
                    .as_ref(),
                b"\x13"
            );
            app.lookup_runtime_sender(0, pane).unwrap().test_process_pty_bytes(
                "\x1b[2J\x1b[H› stashed\r\n────────────────────────\r\n❯ \r\n────────────────────────\r\n".as_bytes(),
            );
        } else {
            assert_eq!(
                step(&mut writes, &response, "clear returned image")
                    .await
                    .as_ref(),
                b"\x01"
            );
            redraw(&app, kind, returned);
            assert_eq!(
                step(&mut writes, &response, "kill returned image")
                    .await
                    .as_ref(),
                b"\x0b"
            );
            redraw(&app, kind, "");
        }
        assert_eq!(
            step(&mut writes, &response, "combined paste").await,
            Bytes::from(format!("\x1b[200~{combined}\x1b[201~"))
        );
        redraw(&app, kind, "[Image #9]old prompt\r\n  new instruction");
        assert_eq!(
            step(&mut writes, &response, "combined enter")
                .await
                .as_ref(),
            b"\r"
        );
        redraw(&app, kind, if developer_edited { returned } else { "" });
        let reply: serde_json::Value =
            serde_json::from_str(&response.recv_timeout(WAIT).unwrap()).unwrap();
        assert_eq!(reply["result"]["type"], "agent_prompted", "{reply}");
        assert_eq!(
            reply["result"]
                .get("discarded_aborted_payload")
                .and_then(|value| value.as_bool()),
            (!developer_edited).then_some(true)
        );
        assert!(writes.try_recv().is_err());
    }
}
