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
    redraw(&app, AgentKind::Claude, "from bus");
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
async fn claude_plain_paste_readiness_expiry_withdraws_without_enter() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let expiry = confirmation_expiry(&method);
    let receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, method);
    next_write(&mut writes).await;
    // The paste is rendered when readiness expires, before any Enter. Hold
    // its flush receipt so observation and expiry cannot race the probe.
    clock(&app).advance(expiry);
    redraw(&app, AgentKind::Claude, "from bus");
    receipt.send(()).unwrap();
    let result = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
    tokio::pin!(result);
    tokio::select! {
        reply = &mut result => {
            panic!("readiness expiry stranded the Bus draft without withdrawal: {}", reply.unwrap());
        }
        bytes = next_write(&mut writes) => {
            assert_eq!(bytes.as_ref(), b"\x01", "withdrawal must begin without Enter");
        }
    };
    redraw(&app, AgentKind::Claude, "");
    let reply = result.await.unwrap();
    assert!(reply.contains("prompt_not_shown"), "{reply}");
    assert!(writes.try_recv().is_err(), "no Enter and no paste replay");
}

#[tokio::test]
async fn claude_plain_paste_readiness_expiry_preserves_foreign_or_edited_input() {
    for buffered in [false, true] {
        let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
        let expiry = confirmation_expiry(&method);
        let receipt = clock(&app).hold_next_write_completion();
        let response = start(&mut app, method);
        next_write(&mut writes).await;
        clock(&app).advance(expiry);
        redraw(
            &app,
            AgentKind::Claude,
            if buffered {
                "from bus"
            } else {
                "foreign draft"
            },
        );
        if buffered {
            let pane = app.state.workspaces[0].tabs[0].root_pane;
            app.lookup_runtime_sender(0, pane)
                .unwrap()
                .try_send_developer_bytes(Bytes::from_static(b" developer edit"))
                .unwrap();
        }
        receipt.send(()).unwrap();
        let mut result = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
        let (reply, unexpected) = tokio::select! {
            reply = &mut result => (reply.unwrap(), None),
            bytes = next_write(&mut writes) => {
                // Complete the unsafe old clear path before reporting its key;
                // the failing probe must not leave a paused writer behind.
                redraw(&app, AgentKind::Claude, "");
                (result.await.unwrap(), Some(bytes))
            }
        };
        assert!(
            unexpected.is_none(),
            "buffered={buffered}: expiry sent {unexpected:?} after ownership was revoked"
        );
        assert!(!reply.contains("agent_prompted"), "{reply}");
        assert!(
            writes.try_recv().is_err(),
            "expiry sent input after ownership was revoked"
        );
    }
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

#[tokio::test]
async fn claude_plain_paste_releases_draft_keys_when_a_fresh_prompt_echo_hides_the_composer() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let receipt = clock(&app).hold_next_write_completion();
    let response = start(&mut app, method);
    next_write(&mut writes).await;
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let render = |text: &str| {
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_process_pty_bytes(text.as_bytes());
    };
    let old_echo = "\x1b[2J\x1b[H> from bus\r\n";
    render(&format!(
        "{old_echo}\r\n────────────────────────\r\n❯ from bus\r\n────────────────────────\r\n"
    ));
    receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    clock(&app).wait_for_enters(1);
    let draft = Bytes::from_static(b"aa  developer draft");
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .try_send_developer_bytes(draft.clone())
        .unwrap();

    let work = "✻ Working… (0s · esc to interrupt)\r\n";
    for frame in [
        format!("{old_echo}{work}"),
        format!("{old_echo}> from bus\r\n{work}ctrl+enter to send now\r\n"),
    ] {
        render(&frame);
        let generation = clock(&app).advance(Duration::from_millis(100));
        clock(&app).wait_for_submission_check(generation);
        assert!(response.try_recv().is_err(), "stale echo or native queue");
        assert!(writes.try_recv().is_err(), "draft keys are still buffered");
    }

    render(&format!("{old_echo}> from bus\r\n{work}"));
    assert!(response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));
    assert_eq!(next_write(&mut writes).await, draft);
    assert!(writes.try_recv().is_err());
}

struct IsolatedDraftEnv {
    _env: crate::utils::test_env::EnvLock,
    root: std::path::PathBuf,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

#[tokio::test]
async fn claude_send_now_hint_blocks_confirmation_from_an_empty_composer() {
    for (hint, expires) in [
        ("ctrl+enter to send now", false),
        ("1 queued · ctrl+enter to send now\r\n✻ Working…", true),
    ] {
        let (mut app, method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
        let expiry = confirmation_expiry(&method);
        let response = start(&mut app, method);
        assert_eq!(
            next_write(&mut writes).await.as_ref(),
            b"\x1b[200~from bus\x1b[201~"
        );
        redraw(&app, AgentKind::Claude, "from bus");
        assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
        clock(&app).wait_for_enters(1);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        app.lookup_runtime_sender(0, pane).unwrap().test_process_pty_bytes(
            format!("\x1b[2J\x1b[H❯ from bus\r\n  {hint}\r\n\r\n────────────────────────\r\n❯ \r\n────────────────────────\r\n").as_bytes(),
        );
        let generation = clock(&app).advance(Duration::from_millis(300));
        clock(&app).wait_for_submission_check(generation);
        assert!(
            response.try_recv().is_err(),
            "native queue is not consumption"
        );
        assert!(writes.try_recv().is_err(), "do not force-send or replay");
        if expires {
            clock(&app).advance(expiry);
            let reply = response.recv_timeout(WAIT).unwrap();
            assert!(reply.contains("The message may have run"), "{reply}");
        } else {
            redraw(&app, AgentKind::Claude, "");
            assert!(response
                .recv_timeout(WAIT)
                .unwrap()
                .contains("agent_prompted"));
        }
        assert!(
            writes.try_recv().is_err(),
            "leave native queue editing to Claude"
        );
    }
}

impl IsolatedDraftEnv {
    fn new() -> Self {
        let env = crate::utils::test_env::env_lock();
        let root = crate::utils::test_temp::unique_temp_path("claude-plain-draft");
        std::fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut previous = Vec::new();
        for (key, value) in [
            ("BUS_DATA_DIR", root.clone()),
            ("CLAUDE_CONFIG_DIR", root.join("claude")),
        ] {
            previous.push((key, std::env::var_os(key)));
            std::env::set_var(key, value);
        }
        Self {
            _env: env,
            root,
            previous,
        }
    }
}

impl Drop for IsolatedDraftEnv {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn claude_interrupt_delivery_discards_returned_aborted_payload_without_stashing() {
    let _env = IsolatedDraftEnv::new();
    let aborted =
        "[Bus message 6 from you (room 1)]\nWrite a detailed glossary directly. Do NOT use tools.";
    let (mut app, mut method, mut writes) = provider_fixture(
        AgentKind::Claude,
        true,
        "[Bus message 6 from you (room 1)]\r\n  Write a detailed glossary directly. Do NOT use\r\n  tools.",
    );
    match &mut method {
        Method::AgentPromptIfIdle(params) => {
            params.text = "[Bus message 8 from you (room 1)]\nM3_ACK".into();
            params.discard_aborted_payload = Some(aborted.into());
        }
        _ => unreachable!(),
    }
    let response = start(&mut app, method);
    // Clear keys discard the soft-wrapped aborted payload without ever sending Ctrl+S (\x13).
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\x01"); // Ctrl+A
    redraw(
        &app,
        AgentKind::Claude,
        "[Bus message 6 from you (room 1)]\r\n  Write a detailed glossary directly. Do NOT use\r\n  tools.",
    );
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\x0b"); // Ctrl+K kills visual wrapped row, leaving "tools."
    redraw(
        &app,
        AgentKind::Claude,
        "[Bus message 6 from you (room 1)]\r\n  tools.",
    );
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\x15"); // Ctrl+U joins
    redraw(&app, AgentKind::Claude, "[Bus message 6 from you (room 1)]");
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\x01"); // Ctrl+A on line 1
    redraw(&app, AgentKind::Claude, "[Bus message 6 from you (room 1)]");
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\x0b"); // Ctrl+K kills line 1
    redraw(&app, AgentKind::Claude, "");

    let paste = next_write(&mut writes).await;
    assert_eq!(
        paste,
        Bytes::from("\x1b[200~[Bus message 8 from you (room 1)]\nM3_ACK\x1b[201~")
    );
    redraw(
        &app,
        AgentKind::Claude,
        "[Bus message 8 from you (room 1)]\r\n  M3_ACK",
    );
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    redraw(&app, AgentKind::Claude, "");

    let reply: serde_json::Value =
        serde_json::from_str(&response.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(reply["result"]["type"], "agent_prompted", "{reply}");
    assert_eq!(
        reply["result"]["discarded_aborted_payload"], true,
        "{reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "no stash/restore keys and no stray bytes after submit; a later Enter sees an empty composer"
    );
}

#[tokio::test]
async fn claude_interrupt_chain_replays_framed_pastes_once_without_stashing() {
    let _env = IsolatedDraftEnv::new();
    let kind = AgentKind::Claude;
    let (mut app, template, mut writes) = provider_fixture(kind, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let mut submitted = String::new();
    for message in [59882, 59884, 59886, 59888] {
        let next = format!("[Bus message {message} from you (room 0)]\nstep {message}");
        let mut method = template.clone();
        let Method::AgentPromptIfIdle(params) = &mut method else {
            unreachable!();
        };
        params.text = next.clone();
        let replaying = !submitted.is_empty();
        let mut returned = Vec::new();
        if replaying {
            let agent = app.agent_info(0, pane).unwrap();
            let response = app.handle_api_request(Request {
                id: "stop".into(),
                method: Method::AgentSendKeys(crate::protocol::api::schema::AgentSendKeysParams {
                    target: agent.pane_id,
                    keys: vec!["esc".into()],
                    expected_terminal_id: Some(agent.terminal_id),
                    expected_activity: None,
                    native_interrupt: None,
                }),
            });
            assert!(response.contains("\"type\":\"ok\""), "{response}");
            assert_eq!(next_write(&mut writes).await.as_ref(), b"\x1b");
            if message != 59884 {
                returned.push("<pasted_content id=\"8bad00\">".to_owned());
            }
            returned.extend(submitted.split('\n').map(str::to_owned));
            if message != 59884 {
                returned.push("</pasted_content id=\"8bad00\">".to_owned());
            }
            redraw(&app, kind, &returned.join("\r\n"));
            params.discard_aborted_payload =
                Some(crate::protocol::api::schema::AbortedPayloadAction::Replay {
                    replay: submitted.clone(),
                });
            submitted = format!("{submitted}\n\n{next}");
        } else {
            submitted = next;
        }
        let response = start(&mut app, method);
        while !returned.is_empty() {
            assert_eq!(
                next_write(&mut writes).await.as_ref(),
                b"\x01",
                "clear the Bus paste, never stash it"
            );
            redraw(&app, kind, &returned.join("\r\n"));
            assert_eq!(next_write(&mut writes).await.as_ref(), b"\x0b");
            returned.remove(0);
            while returned.first().is_some_and(String::is_empty) {
                returned.remove(0);
            }
            if returned.is_empty() {
                redraw(&app, kind, "");
            } else {
                redraw(&app, kind, &format!("\r\n{}", returned.join("\r\n")));
                assert_eq!(next_write(&mut writes).await.as_ref(), b"\x15");
                redraw(&app, kind, &returned.join("\r\n"));
            }
        }
        assert_eq!(
            next_write(&mut writes).await,
            Bytes::from(format!("\x1b[200~{submitted}\x1b[201~"))
        );
        redraw(&app, kind, &submitted.replace('\n', "\r\n"));
        assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
        redraw(&app, kind, "");
        let reply: serde_json::Value =
            serde_json::from_str(&response.recv_timeout(WAIT).unwrap()).unwrap();
        assert_eq!(reply["result"]["type"], "agent_prompted", "{reply}");
        assert_eq!(
            reply["result"]["discarded_aborted_payload"]
                .as_bool()
                .unwrap_or_default(),
            replaying,
            "{reply}"
        );
        assert!(writes.try_recv().is_err());
    }
}

#[tokio::test]
async fn claude_interrupt_delivery_discards_returned_collapsed_payload() {
    let _env = IsolatedDraftEnv::new();
    claude_collapsed_interrupt(false).await;
}

#[tokio::test]
async fn claude_interrupt_delivery_preserves_an_unrelated_collapsed_draft() {
    let _env = IsolatedDraftEnv::new();
    claude_collapsed_interrupt(true).await;
}

async fn claude_collapsed_interrupt(typed_draft: bool) {
    let aborted = [
        "[Bus message 6 from peer]",
        "",
        "Read the assignment and report the result.",
        "",
        "Include evidence with the report.",
    ]
    .join("\n");
    let collapsed = "[Pasted text #12 +4 lines]";
    let kind = AgentKind::Claude;
    let (mut app, mut lead_method, mut writes) = provider_fixture(kind, true, "");
    let mut follow_method = lead_method.clone();
    let Method::AgentPromptIfIdle(lead_params) = &mut lead_method else {
        unreachable!();
    };
    lead_params.text = aborted.clone();
    let lead_response = start(&mut app, lead_method);
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from(format!("\x1b[200~{aborted}\x1b[201~"))
    );
    redraw(&app, kind, "[Pasted text #11 +4 lines]");
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    redraw(&app, kind, "");
    assert!(lead_response
        .recv_timeout(WAIT)
        .unwrap()
        .contains("agent_prompted"));

    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    if typed_draft {
        let draft = Bytes::from_static(b"an unrelated developer input");
        runtime.try_send_developer_bytes(draft.clone()).unwrap();
        assert_eq!(next_write(&mut writes).await, draft);
        redraw(&app, kind, collapsed);
    }
    let agent = app.agent_info(0, pane).unwrap();
    let stop = Method::AgentSendKeys(crate::protocol::api::schema::AgentSendKeysParams {
        target: agent.pane_id,
        keys: vec!["esc".into()],
        expected_terminal_id: Some(agent.terminal_id),
        expected_activity: None,
        native_interrupt: None,
    });
    let response = app.handle_api_request(Request {
        id: "stop".into(),
        method: stop,
    });
    assert!(response.contains("\"type\":\"ok\""), "{response}");
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\x1b");
    redraw(&app, kind, collapsed);

    let Method::AgentPromptIfIdle(params) = &mut follow_method else {
        unreachable!();
    };
    params.discard_aborted_payload = Some(aborted.into());
    let response = start(&mut app, follow_method);

    let first_key = next_write(&mut writes).await;
    if typed_draft {
        assert_eq!(first_key.as_ref(), b"\x13", "stash the developer's chip");
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_process_pty_bytes(
                "\x1b[2J\x1b[H› stashed\r\n────────────────────────\r\n❯ \r\n────────────────────────\r\n"
                    .as_bytes(),
            );
    } else {
        assert_eq!(first_key.as_ref(), b"\x01", "clear the returned chip");
        redraw(&app, kind, collapsed);
        assert_eq!(next_write(&mut writes).await.as_ref(), b"\x0b");
        redraw(&app, kind, "");
    }

    assert_eq!(
        next_write(&mut writes).await.as_ref(),
        b"\x1b[200~from bus\x1b[201~"
    );
    redraw(&app, kind, "from bus");
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    redraw(&app, kind, if typed_draft { collapsed } else { "" });

    let reply: serde_json::Value =
        serde_json::from_str(&response.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(reply["result"]["type"], "agent_prompted", "{reply}");
    if typed_draft {
        assert!(reply["result"].get("discarded_aborted_payload").is_none());
        assert!(app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .visible_text()
            .contains(collapsed));
    } else {
        assert_eq!(reply["result"]["discarded_aborted_payload"], true);
    }
    assert!(writes.try_recv().is_err());
}

#[tokio::test]
async fn claude_interrupt_delivery_still_preserves_unrelated_typed_draft() {
    let _env = IsolatedDraftEnv::new();
    let aborted = "[Bus message 6 from you (room 1)]\nM2_STEER_MARKER_A report alpha";
    let draft = "aa  developer draft";
    let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Claude, true, draft);
    match &mut method {
        Method::AgentPromptIfIdle(params) => {
            params.text = "[Bus message 8 from you (room 1)]\nM3_ACK".into();
            params.discard_aborted_payload = Some(aborted.into());
        }
        _ => unreachable!(),
    }
    let response = start(&mut app, method);
    // Unrelated developer draft takes #494's Ctrl+S stash path, not aborted-payload discard.
    let first_key = next_write(&mut writes).await;
    assert_eq!(
        first_key.as_ref(),
        b"\x13",
        "typed developer draft must still be stashed with Ctrl+S"
    );
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(
        "\x1b[2J\x1b[H› stashed\r\n────────────────────────\r\n❯ \r\n────────────────────────\r\n"
            .as_bytes(),
    );
    let paste = next_write(&mut writes).await;
    assert_eq!(
        paste,
        Bytes::from("\x1b[200~[Bus message 8 from you (room 1)]\nM3_ACK\x1b[201~")
    );
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(
            "\x1b[2J\x1b[H› stashed\r\n────────────────────────\r\n❯ [Bus message 8 from you (room 1)]\r\n  M3_ACK\r\n────────────────────────\r\n"
                .as_bytes(),
        );
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    // Native stash pops the developer draft back into the composer after submit.
    redraw(&app, AgentKind::Claude, draft);

    let reply: serde_json::Value =
        serde_json::from_str(&response.recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(reply["result"]["type"], "agent_prompted", "{reply}");
    assert!(
        reply["result"].get("discarded_aborted_payload").is_none(),
        "real draft was preserved, not discarded: {reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "native stash restore needs no extra writes after Enter"
    );
    let final_screen = app.lookup_runtime_sender(0, pane).unwrap().visible_text();
    assert!(
        final_screen.contains(draft),
        "unrelated developer draft must remain restored in the composer: {final_screen:?}"
    );
}

#[tokio::test]
async fn claude_working_steer_still_refuses_with_turn_busy_and_leaves_composer_empty() {
    let _env = IsolatedDraftEnv::new();
    let (mut app, mut method, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(
            "\x1b[2J\x1b[H✻ Puttering… (16s · esc to interrupt)\r\n────────────────────────\r\n❯ \r\n────────────────────────\r\n"
                .as_bytes(),
        );
    match &mut method {
        Method::AgentPromptIfIdle(params) => {
            params.text = "[Bus message 14 from you (room 1)]\nM2_WORKING_STEER_B".into();
            params.steer = true;
        }
        _ => unreachable!(),
    }
    let reply: serde_json::Value =
        serde_json::from_str(&start(&mut app, method).recv_timeout(WAIT).unwrap()).unwrap();
    assert_eq!(
        reply["error"]["code"],
        crate::protocol::api::AGENT_CLAUDE_TURN_BUSY,
        "{reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "Working steer must write nothing"
    );
}
