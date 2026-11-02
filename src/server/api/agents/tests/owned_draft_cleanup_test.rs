use super::*;
use crate::terminal::runtime::{InputObservation, PromptNotShown};

pub(super) async fn write_or_done<T: std::fmt::Debug>(
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

pub(super) async fn finish_response<T>(
    response: (
        impl std::future::Future<Output = Result<T, tokio::task::JoinError>>,
        Option<T>,
    ),
) -> T {
    match response {
        (_, Some(result)) => result,
        (response, None) => response.await.unwrap(),
    }
}

pub(super) async fn fail_custom_enter_receipt(
    app: &mut App,
    method: Method,
    text: &str,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    render: impl FnOnce(&App),
) -> String {
    let native_clock = clock(app);
    let paste_receipt = native_clock.hold_next_write_completion();
    let response = start(app, method);
    assert_eq!(
        next_write(writes).await,
        Bytes::from(format!("\x1b[200~{text}\x1b[201~"))
    );
    render(app);
    let enter_receipt = native_clock.fail_next_write_completion(std::io::ErrorKind::TimedOut);
    paste_receipt.send(()).unwrap();
    assert_eq!(next_write(writes).await, Bytes::from_static(b"\r"));
    enter_receipt.send(()).unwrap();
    let reply = response.recv_timeout(WAIT).unwrap();
    assert!(reply.contains("timeout"), "{reply}");
    assert!(
        !reply.contains("agent_prompted"),
        "lost Enter receipt is not acceptance: {reply}"
    );
    assert!(
        !reply.contains("agent_prompt_not_shown"),
        "Enter may have run: {reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "one paste and one Enter; no automatic cleanup or replay"
    );
    reply
}

fn native_screen(app: &App) -> String {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane).unwrap().visible_ansi()
}

async fn withdraw_inline_draft(edit_after_first_kill: Option<&str>) {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Codex, true, "");
    let Method::AgentPromptIfIdle(mut params) = method else {
        unreachable!()
    };
    const TEXT: &str = "header\nmiddle words\nlast";
    params.text = TEXT.into();
    let old_reply = fail_custom_enter_receipt(
        &mut app,
        Method::AgentPromptIfIdle(params),
        TEXT,
        &mut writes,
        |app| {
            redraw(app, AgentKind::Codex, "header\r\n  middle words\r\n  last");
        },
    )
    .await;
    // A fresh lease recovers the old uncertain paste, then deliberately refuses new input.
    let completion = recovery_without_new_input(&app, None);
    let mut response = (
        tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap()),
        None,
    );
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
        let saved = native_screen(&app);
        let result = match write_or_done(&mut writes, &mut response).await {
            Err(result) => result,
            Ok(key) => {
                clock(&app).advance(Duration::from_secs(2));
                if response.1.is_none() {
                    let _ = response.0.await;
                }
                panic!("cleanup sent {key:?} after a developer edit");
            }
        };
        let error = result.unwrap_err();
        assert!(
            PromptNotShown::composer_busy(&error),
            "old cleanup must refuse a foreign edit: {error}"
        );
        assert_eq!(
            native_screen(&app),
            saved,
            "the exact edited native frame must remain unchanged"
        );
        assert!(
            writes.try_recv().is_err(),
            "never delete or submit a developer edit"
        );
        return;
    }
    // Independent line-local editor, including Ctrl+U joining the empty final line.
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
    let error = finish_response(response).await.unwrap_err();
    assert!(
        PromptNotShown::is(&error) && !PromptNotShown::composer_busy(&error),
        "old recovery finished; only the fresh unavailable input is refused: {error}"
    );
    assert!(
        !native_screen(&app).contains("header"),
        "no old multiline fragment remains"
    );
    assert!(
        !old_reply.contains("agent_prompted") && !old_reply.contains("agent_prompt_not_shown"),
        "clearing later does not erase old Enter uncertainty: {old_reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "one old paste/Enter; no new input or replay"
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

/// Grok 1.0.70 draws a labelled lower border and an Enter:send footer.
/// Continuation rows keep three spaces inside the frame; `input` strips them.
fn grok_native_frame(lines: &[&str]) -> String {
    const INNER: usize = 60;
    let mut rows = Vec::with_capacity(lines.len() + 3);
    rows.push(format!("  ╭{}╮", "─".repeat(INNER)));
    let source: Vec<String> = if lines.is_empty() {
        vec![" ❯".to_owned()]
    } else {
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                if index == 0 {
                    format!(" ❯ {line}")
                } else {
                    format!("   {line}")
                }
            })
            .collect()
    };
    for text in source {
        assert!(
            text.chars().count() <= INNER,
            "frame row wider than the composer: {text}"
        );
        rows.push(format!("  │{text:<INNER$}│"));
    }
    rows.push(format!(
        "  ╰{} Grok 4.7 (high) · always-approve ─╯",
        "─".repeat(8)
    ));
    rows.push(
        "  Enter:send  │  Shift+Enter/Opt+Enter:newline  │  Shift+Tab:mode  │  Ctrl+.:shortcuts"
            .into(),
    );
    format!("\x1b[2J\x1b[H{}", rows.join("\r\n"))
}

fn paint_grok(app: &App, lines: &[&str]) {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(grok_native_frame(lines).as_bytes());
}

async fn withdraw_grok_owned_draft(edit_after_first_kill: Option<&[&str]>) {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Grok, true, "");
    paint_grok(&app, &[]);
    let Method::AgentPromptIfIdle(mut params) = method else {
        unreachable!()
    };
    const TEXT: &str = "header\nmiddle words\nlast";
    params.text = TEXT.into();
    let old_reply = fail_custom_enter_receipt(
        &mut app,
        Method::AgentPromptIfIdle(params),
        TEXT,
        &mut writes,
        |app| {
            paint_grok(app, &["header", "middle words", "last"]);
        },
    )
    .await;
    let completion = recovery_without_new_input(&app, None);
    let mut response = (
        tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap()),
        None,
    );
    assert_eq!(
        write_or_done(&mut writes, &mut response).await.unwrap(),
        Bytes::from_static(b"\x01")
    );
    paint_grok(&app, &["header", "middle words", "last"]);
    assert_eq!(
        write_or_done(&mut writes, &mut response).await.unwrap(),
        Bytes::from_static(b"\x0b")
    );
    if let Some(edit) = edit_after_first_kill {
        paint_grok(&app, edit);
        let saved = native_screen(&app);
        let result = match write_or_done(&mut writes, &mut response).await {
            Err(result) => result,
            Ok(key) => {
                clock(&app).advance(Duration::from_secs(2));
                if response.1.is_none() {
                    let _ = response.0.await;
                }
                panic!("cleanup sent {key:?} after a developer edit");
            }
        };
        let error = result.unwrap_err();
        assert!(
            PromptNotShown::composer_busy(&error),
            "old cleanup must refuse a foreign edit: {error}"
        );
        assert_eq!(
            native_screen(&app),
            saved,
            "the exact Grok edit must remain unchanged"
        );
        assert!(
            writes.try_recv().is_err(),
            "never delete or submit a developer edit"
        );
        return;
    }
    paint_grok(&app, &["header", "middle words"]);
    for (key, remaining) in [
        (b"\x15".as_slice(), &["header", "middle words"][..]),
        (b"\x01".as_slice(), &["header", "middle words"][..]),
        (b"\x0b".as_slice(), &["header"][..]),
        (b"\x15".as_slice(), &["header"][..]),
        (b"\x01".as_slice(), &["header"][..]),
        (b"\x0b".as_slice(), &[][..]),
    ] {
        assert_eq!(
            write_or_done(&mut writes, &mut response)
                .await
                .unwrap()
                .as_ref(),
            key,
            "while the composer still showed {remaining:?}"
        );
        paint_grok(&app, remaining);
    }
    let error = finish_response(response).await.unwrap_err();
    assert!(
        PromptNotShown::is(&error) && !PromptNotShown::composer_busy(&error),
        "old recovery finished; fresh input remains unavailable: {error}"
    );
    assert!(
        !native_screen(&app).contains("header"),
        "no old Grok line remains"
    );
    assert!(
        !old_reply.contains("agent_prompted") && !old_reply.contains("agent_prompt_not_shown"),
        "clearing is not old-request acceptance or definite non-delivery: {old_reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "one old paste/Enter; no new input or replay"
    );
}

#[tokio::test]
async fn grok_unconfirmed_multiline_draft_is_withdrawn_instead_of_left_behind() {
    withdraw_grok_owned_draft(None).await;
}

#[tokio::test]
async fn grok_developer_edit_during_withdrawal_is_preserved() {
    withdraw_grok_owned_draft(Some(&["header", "middle words + developer edit"])).await;
}

#[tokio::test]
async fn grok_developer_deleted_line_is_not_treated_as_owned_progress() {
    withdraw_grok_owned_draft(Some(&["header"])).await;
}

/// Enter during a live turn parks the prompt in Grok's own queue. The
/// composer is gone and "▾ Queued N" is under Tasks. That is acceptance:
/// Bus must not withdraw the prompt or report the submit as unconfirmed.
#[tokio::test]
async fn grok_queued_submission_is_accepted_instead_of_withdrawn() {
    let (mut app, method, mut writes) = provider_fixture(AgentKind::Grok, true, "");
    paint_grok(&app, &[]);
    let expiry = confirmation_expiry(&method);
    let Method::AgentPromptIfIdle(params) = method else {
        unreachable!()
    };
    let response = start(&mut app, Method::AgentPromptIfIdle(params));
    assert_eq!(
        next_write(&mut writes).await,
        Bytes::from_static(b"\x1b[200~from bus\x1b[201~")
    );
    paint_grok(&app, &["from bus"]);
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    clock(&app).wait_for_enters(1);
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes("\x1b[2J\x1b[H   ▾ Queued 1\r\n   from bus\r\n".as_bytes());
    clock(&app).advance(expiry);
    let reply = response.recv_timeout(WAIT).unwrap();
    assert!(
        reply.contains("agent_prompted"),
        "a Grok-queued prompt is an accepted submission: {reply}"
    );
    assert!(
        !reply.contains("may have run"),
        "acceptance must not be reported as an unconfirmed withdraw: {reply}"
    );
    assert!(
        writes.try_recv().is_err(),
        "an accepted queue must not be cleared or retyped"
    );
}

/// Acceptance in old pending-paste recovery stops cleanup without a clear key.
#[tokio::test]
async fn acceptance_seen_by_first_cleanup_check_is_not_reported_as_cleared() {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    for accepted in [InputObservation::Empty, InputObservation::Accepted] {
        let (app, _, mut writes) = provider_fixture(AgentKind::Claude, true, "");
        let native_clock = clock(&app);
        native_clock.pause();
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let runtime = app.lookup_runtime_sender(0, pane).unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let observed_ready = ready.clone();
        let recovery_checks = Arc::new(AtomicUsize::new(0));
        let observed_checks = recovery_checks.clone();
        let paste_receipt = native_clock.hold_next_write_completion();
        let completion = runtime
            .queue_observed_input_submission(
                Bytes::from_static(b"PROBE"),
                Bytes::from_static(b"\r"),
                vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
                Duration::ZERO,
                None,
                move |screen| {
                    if observed_ready.load(Ordering::SeqCst) {
                        if observed_checks.fetch_add(1, Ordering::SeqCst) == 0 {
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
        runtime.test_process_pty_bytes(b"\x1b[2J\x1b[HPROBE");
        let enter_receipt = native_clock.fail_next_write_completion(std::io::ErrorKind::TimedOut);
        paste_receipt.send(()).unwrap();
        assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
        enter_receipt.send(()).unwrap();
        let old_error = completion.recv_timeout(WAIT).unwrap().unwrap_err();
        assert!(
            !PromptNotShown::is(&old_error),
            "old Enter remains uncertain: {old_error}"
        );
        ready.store(true, Ordering::SeqCst);
        let recovery = recovery_without_new_input(&app, None);
        let error = recovery.recv_timeout(WAIT).unwrap().unwrap_err();
        assert!(
            PromptNotShown::is(&error) && !PromptNotShown::composer_busy(&error),
            "accepted old work is resolved before refusing fresh input: {error}"
        );
        assert_eq!(
            recovery_checks.load(Ordering::SeqCst),
            2,
            "recovery first saw Pending, then clear_step observed acceptance"
        );
        assert!(
            writes.try_recv().is_err(),
            "no cleanup key, old replay or fresh input after acceptance"
        );
    }
}

#[tokio::test]
async fn positive_acceptance_during_withdrawal_stops_further_clear_keys() {
    let (app, _, mut writes) = provider_fixture(AgentKind::Claude, true, "");
    let native_clock = clock(&app);
    native_clock.pause();
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    let paste_receipt = native_clock.hold_next_write_completion();
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
    let enter_receipt = native_clock.fail_next_write_completion(std::io::ErrorKind::TimedOut);
    paste_receipt.send(()).unwrap();
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\r"));
    enter_receipt.send(()).unwrap();
    let old_error = completion.recv_timeout(WAIT).unwrap().unwrap_err();
    assert!(
        !PromptNotShown::is(&old_error),
        "old Enter remains uncertain: {old_error}"
    );
    let recovery = recovery_without_new_input(&app, None);
    assert_eq!(next_write(&mut writes).await, Bytes::from_static(b"\x01"));
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[HWORKING");
    let error = recovery.recv_timeout(WAIT).unwrap().unwrap_err();
    assert!(
        PromptNotShown::is(&error) && !PromptNotShown::composer_busy(&error),
        "old work resolves before fresh unavailable input: {error}"
    );
    assert!(
        writes.try_recv().is_err(),
        "no cleanup key after acceptance, duplicate Enter or fresh paste"
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
    let paste_receipt = clock(&app).fail_next_write_completion(std::io::ErrorKind::TimedOut);
    let response = start(&mut app, method);
    let started = clock(&app).now();
    let mut response = (
        tokio::task::spawn_blocking(move || response.recv_timeout(WAIT).unwrap()),
        None,
    );
    let paste = next_write(&mut writes).await;
    assert!(paste.starts_with(b"\x1b[200~item 01"));
    redraw(&app, AgentKind::Codex, &render(&text));
    let mut acknowledge_key = clock(&app).hold_next_write_completion();
    clock(&app).advance(expiry);
    let withdrawal_started = clock(&app).now();
    paste_receipt.send(()).unwrap();
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
        // Hold the write acknowledgement until the fake editor has applied it
        // and advanced time. Redraw wakes alone must not queue the next key.
        clock(&app).advance(Duration::from_millis(25));
        let next_acknowledgement = clock(&app).hold_next_write_completion();
        acknowledge_key.send(()).unwrap();
        acknowledge_key = next_acknowledgement;
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
            reply.contains("agent_prompt_not_shown"),
            "{keys} keys sent; clearing before any Enter is definite non-delivery; left {:?}; reply={reply}",
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
    }
    assert!(clock(&app).now() <= started + hard_budget);
    assert!(
        writes.try_recv().is_err(),
        "one paste, no Enter and no keys after completion"
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
            expected_turn_id: None,
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
