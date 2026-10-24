use super::*;

async fn write_or_done<T: std::fmt::Debug>(
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    response: &mut tokio::task::JoinHandle<T>,
) -> Result<Bytes, T> {
    tokio::select! {
        bytes = next_write(writes) => Ok(bytes),
        result = response => Err(result.unwrap()),
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
    let mut response = tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap());
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
                let _ = response.await;
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
    let reply = response.await.unwrap();
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
