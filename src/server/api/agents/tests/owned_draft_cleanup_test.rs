use super::*;

async fn write_or_done<T: std::fmt::Debug>(
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    response: &mut (
        impl std::future::Future<Output = Result<T, tokio::task::JoinError>> + Unpin,
        Option<T>,
    ),
) -> Result<Bytes, T> {
    let (response, completed) = response;
    if completed.is_none() {
        tokio::select! {
            bytes = next_write(writes) => return Ok(bytes),
            result = response => *completed = Some(result.unwrap()),
        }
    }
    // A key may have been queued before completion while the fake provider was
    // descheduled. Apply every such input event before returning the reply.
    // Keep the completed result: a JoinHandle must never be polled twice.
    match writes.try_recv() {
        Ok(bytes) => Ok(bytes),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => Err(completed.take().unwrap()),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
            panic!("native input channel closed")
        }
    }
}

async fn withdraw_inline_draft(edit_after_first_kill: Option<&str>) {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let expiry = confirmation_expiry(&method);
    let Method::AgentPromptIfIdle(mut params) = method else {
        unreachable!()
    };
    params.text = "header\nmiddle words\nlast".into();
    let response = start(&mut app, Method::AgentPromptIfIdle(params));
    let mut response = (
        tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap()),
        None,
    );
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from_static(b"\x1b[200~header\nmiddle words\nlast\x1b[201~")
    );
    redraw(&app, AgentKind::Codex, "header\r\n  middle words\r\n  last");
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).wait_for_enters(1);
    clock(&app).advance(expiry);
    assert_eq!(
        write_or_done(&mut writes, &mut response).await.unwrap(),
        Bytes::from_static(b"\x01")
    );
    redraw(&app, AgentKind::Codex, "header\r\n  middle words\r\n  last");
    assert_eq!(
        write_or_done(&mut writes, &mut response).await.unwrap(),
        Bytes::from_static(b"\x0b")
    );
    if let Some(edit) = edit_after_first_kill {
        redraw(&app, AgentKind::Codex, edit);
        let reply = match write_or_done(&mut writes, &mut response).await {
            Err(reply) => reply,
            Ok(key) => {
                clock(&app).advance(Duration::from_secs(2));
                let _ = response.0.await;
                panic!("cleanup sent {key:?} after a developer edit");
            }
        };
        assert!(reply.contains("could not be safely cleared"), "{reply}");
        assert!(
            writes.try_recv().is_err(),
            "never delete or submit a developer edit"
        );
        return;
    }
    // These are the provider's line-editing effects, including Ctrl+U joining
    // an empty last line to the preceding line. Each key is a separate event.
    redraw(&app, AgentKind::Codex, "header\r\n  middle words");
    for (key, remaining) in [
        (b"\x15".as_slice(), "header\r\n  middle words"),
        (b"\x01".as_slice(), "header\r\n  middle words"),
        (b"\x0b".as_slice(), "header"),
        (b"\x15".as_slice(), "header"),
        (b"\x01".as_slice(), "header"),
        (b"\x0b".as_slice(), ""),
    ] {
        assert_eq!(
            write_or_done(&mut writes, &mut response)
                .await
                .unwrap()
                .as_ref(),
            key
        );
        redraw(&app, AgentKind::Codex, remaining);
    }
    let reply = response.0.await.unwrap();
    assert!(reply.contains("remaining draft was cleared"), "{reply}");
    assert!(
        !reply.contains("agent_prompted"),
        "clearing is not acceptance: {reply}"
    );
    assert!(
        !reply.contains("agent_prompt_not_shown"),
        "Enter may have run: {reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "one paste, one Enter, no replay"
    );
}

#[tokio::test]
async fn codex_unaccepted_multiline_draft_is_fully_withdrawn_after_line_kills() {
    withdraw_inline_draft(None).await;
}

#[tokio::test]
async fn an_edit_after_partial_withdrawal_is_preserved() {
    withdraw_inline_draft(Some("header\r\n  middle words + developer edit")).await;
}

#[tokio::test]
async fn a_developer_deleted_line_is_not_mistaken_for_bus_partial_progress() {
    withdraw_inline_draft(Some("header")).await;
}

/// The composer emptying after Enter is this module's acceptance signal.
/// Seen before any cleanup key, it must not be reported as Bus clearing it.
#[tokio::test]
async fn acceptance_seen_by_first_cleanup_check_is_not_reported_as_cleared() {
    use crate::terminal::runtime::InputObservation;
    for accepted in [InputObservation::Empty, InputObservation::Accepted] {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (app, _, mut writes) = provider_fixture(AgentKind::Claude, true, "");
        let clock = clock(&app);
        clock.pause();
        let start_at = clock.now();
        let observed_clock = clock.clone();
        let expired_calls = AtomicUsize::new(0);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let completion = app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .queue_observed_input_submission(
                Bytes::from_static(b"PROBE"),
                Bytes::from_static(b"\r"),
                vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
                Duration::ZERO,
                None,
                move |screen| {
                    if observed_clock.now() >= start_at + Duration::from_millis(2000) {
                        // First post-deadline check (confirm) still sees the
                        // draft; the provider then accepts before cleanup looks.
                        if expired_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            InputObservation::Pending
                        } else {
                            accepted
                        }
                    } else if screen.contains("PROBE") {
                        InputObservation::Pending
                    } else {
                        InputObservation::Empty
                    }
                },
            )
            .unwrap();
        assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"PROBE"));
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_process_pty_bytes(b"\x1b[2J\x1b[HPROBE");
        assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
        clock.wait_for_enters(1);
        clock.advance(Duration::from_millis(2100));
        let result = completion.recv_timeout(WAIT).unwrap();
        assert!(writes.try_recv().is_err(), "no cleanup key was sent");
        assert!(
            result.is_ok(),
            "composer emptied after Enter with no Bus clear key, yet reported: {result:?}"
        );
    }
}

#[tokio::test]
async fn positive_acceptance_during_withdrawal_stops_further_clear_keys() {
    use crate::terminal::runtime::InputObservation;
    let (app, _, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let clock = clock(&app);
    clock.pause();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    let completion = runtime
        .queue_observed_input_submission(
            Bytes::from_static(b"BRIEF"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            None,
            |screen| {
                if screen.contains("WORKING") {
                    InputObservation::Accepted
                } else if screen.contains("BRIEF") {
                    InputObservation::Pending
                } else {
                    InputObservation::Empty
                }
            },
        )
        .unwrap();
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"BRIEF"));
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[HBRIEF");
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock.wait_for_enters(1);
    clock.advance(Duration::from_millis(2100));
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\x01"));
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[HWORKING");
    assert!(
        completion.recv_timeout(WAIT).unwrap().is_ok(),
        "a visible turn started"
    );
    assert!(
        writes.try_recv().is_err(),
        "no cleanup key after acceptance"
    );
}

#[tokio::test]
async fn whitespace_edits_after_partial_withdrawal_are_preserved() {
    withdraw_inline_draft(Some("header\r\n  middle  words")).await;
}

async fn long_inline_withdrawal(line_count: usize, separator: &str, ignore_keys: bool) {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .resize((line_count + 10) as u16, 100, 0, 0);
    let Method::AgentPromptIfIdle(mut params) = method else {
        unreachable!()
    };
    let lines: Vec<String> = (1..=line_count).map(|n| format!("item {n:02}")).collect();
    let mut text = lines.join("\n");
    params.text = lines.join(separator);
    let method = Method::AgentPromptIfIdle(params);
    let expiry = confirmation_expiry(&method);
    let hard_budget = crate::protocol::api::submission::method_budget(&method).unwrap();
    let render = |text: &str| text.trim_end_matches('\n').replace('\n', "\r\n  ");
    let response = start(&mut app, method);
    let started = clock(&app).now();
    let mut response = (
        tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap()),
        None,
    );
    let paste = next_write(&mut writes).await;
    assert!(paste.starts_with(b"\x1b[200~item 01"));
    redraw(&app, AgentKind::Codex, &render(&text));
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).wait_for_enters(1);
    clock(&app).advance(expiry);
    let withdrawal_started = clock(&app).now();
    // Independent line-local fake editor; cursor starts at the end of paste.
    let mut cursor = text.len();
    let mut keys = 0;
    let reply = loop {
        let key = match write_or_done(&mut writes, &mut response).await {
            Ok(key) => key,
            Err(reply) => break reply,
        };
        assert!(
            clock(&app).now() < started + hard_budget,
            "clear key after hard deadline"
        );
        keys += 1;
        let start = text[..cursor].rfind('\n').map_or(0, |at| at + 1);
        let end = text[cursor..]
            .find('\n')
            .map_or(text.len(), |at| cursor + at);
        match key.as_ref() {
            b"\x01" if !ignore_keys => cursor = start,
            b"\x0b" if !ignore_keys => {
                let end = if cursor == end && end < text.len() {
                    end + 1
                } else {
                    end
                };
                text.replace_range(cursor..end, "");
            }
            b"\x15" if !ignore_keys => {
                let start = if cursor == start {
                    start.saturating_sub(1)
                } else {
                    start
                };
                text.replace_range(start..cursor, "");
                cursor = start;
            }
            b"\x01" | b"\x0b" | b"\x15" => {}
            other => panic!("unexpected key {other:?}; never paste again or send late Enter"),
        }
        // An ignored key produces no provider output. A synthetic redraw would
        // add a second clock wake and let the writer run ahead of this editor.
        if !ignore_keys {
            redraw(&app, AgentKind::Codex, &render(&text));
        }
        // Production spaces editing keys 25ms apart (clear_step sleep).
        clock(&app).advance(Duration::from_millis(25));
    };
    if ignore_keys {
        assert!(
            reply.contains("no owned-draft withdrawal progress"),
            "{reply}"
        );
        assert!(clock(&app).now() - withdrawal_started <= Duration::from_millis(1100));
        assert_eq!(text, lines.join("\n"));
        assert!(!reply.contains("remaining draft was cleared"), "{reply}");
    } else {
        assert!(
            reply.contains("remaining draft was cleared"),
            "{keys} keys sent; left {:?}; reply={reply}",
            render(&text)
        );
        assert!(
            text.trim_end_matches('\n').is_empty(),
            "fragment left: {text:?}"
        );
        assert!(
            !reply.contains("agent_prompted"),
            "clearing is not acceptance: {reply}"
        );
        assert!(
            !reply.contains("agent_prompt_not_shown"),
            "Enter may have run: {reply}"
        );
    }
    assert!(clock(&app).now() <= started + hard_budget);
    assert!(
        writes.try_recv().is_err(),
        "one paste/Enter and no keys after completion"
    );
}

#[tokio::test]
async fn a_long_inline_draft_is_fully_withdrawn_with_real_key_spacing() {
    long_inline_withdrawal(16, "\n", false).await;
}

#[tokio::test]
async fn sixty_four_line_drafts_allow_withdrawal_with_lf_crlf_and_cr() {
    for separator in ["\n", "\r\n", "\r"] {
        long_inline_withdrawal(64, separator, false).await;
    }
}

#[tokio::test]
async fn an_unresponsive_long_draft_stops_on_no_progress_without_spending_its_size_budget() {
    long_inline_withdrawal(64, "\n", true).await;
}

#[tokio::test]
async fn guarded_delivery_preserves_an_existing_unowned_draft() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        preserve_existing_draft(kind, true).await;
        if kind == AgentKind::Codex {
            preserve_existing_draft(kind, false).await;
        }
    }
}

#[test]
fn native_method_budget_reserves_line_edit_cost_for_long_drafts() {
    let lines = (0..64).map(|_| "item").collect::<Vec<_>>();
    let mut budgets = Vec::new();
    for separator in ["\n", "\r\n", "\r"] {
        let method = Method::AgentPromptIfIdle(AgentPromptIfIdleParams {
            target: "pane".into(),
            text: lines.join(separator),
            expected_terminal_id: "terminal".into(),
            expected_pane_id: "pane".into(),
            expected_agent: "claude".into(),
            expected_session_id: "session".into(),
            steer: false,
        });
        let budget = crate::protocol::api::submission::method_budget(&method).unwrap();
        assert!(
            budget >= Duration::from_millis(300 + 2000 + 64 * 3 * 25),
            "hard budget cannot fit the owned line edits: {budget:?}"
        );
        assert!(
            budget <= Duration::from_secs(20),
            "line edit cost must use lines, not payload bytes: {budget:?}"
        );
        budgets.push(budget);
    }
    assert!(
        budgets.windows(2).all(|pair| pair[0] == pair[1]),
        "CRLF is one line separator: {budgets:?}"
    );
}

/// Force the consumer to be descheduled until a write and completion are ready.
/// The completion future enqueues the key while select! is polling it, so even
/// a biased select that checks writes first cannot close this ordering gap.
#[tokio::test]
async fn a_write_queued_before_completion_is_applied_before_the_reply() {
    let (sender, mut writes) = tokio::sync::mpsc::channel(1);
    let mut response = (
        std::future::poll_fn(move |_| {
            sender.try_send(Bytes::from_static(b"\x01")).unwrap();
            std::task::Poll::Ready(Ok("complete"))
        }),
        None,
    );
    assert_eq!(
        write_or_done(&mut writes, &mut response).await,
        Ok(Bytes::from_static(b"\x01")),
        "apply the key queued before completion, even when completion wins select!"
    );
    assert_eq!(
        write_or_done(&mut writes, &mut response).await,
        Err("complete")
    );
    assert!(writes.try_recv().is_err(), "no unprocessed writes");
}
