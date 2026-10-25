use super::*;

async fn failed_write_withdraws_owned_draft(enter: bool) {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let mut release = (!enter).then(|| clock.fail_next_write_completion(io::ErrorKind::TimedOut));
    let completion = runtime
        .queue_observed_input_submission(
            Bytes::from_static(b"owned brief"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            None,
            |screen| {
                if screen.contains("owned brief") {
                    InputObservation::Pending
                } else {
                    InputObservation::Empty
                }
            },
        )
        .unwrap();
    assert_eq!(
        writes.recv().await.unwrap(),
        Bytes::from_static(b"owned brief")
    );
    if enter {
        release = Some(clock.fail_next_write_completion(io::ErrorKind::TimedOut));
    }
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[Howned brief");
    if enter {
        assert_eq!(writes.recv().await.unwrap(), Bytes::from_static(b"\r"));
    }
    clock.advance(Duration::from_millis(2100));
    release.unwrap().send(()).unwrap();
    let mut response = tokio::task::spawn_blocking(move || {
        completion
            .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
            .unwrap()
    });
    let key = tokio::select! {
        key=writes.recv()=>key.unwrap(),
        result=&mut response=>panic!("write failure returned before withdrawing the visible owned draft: {result:?}"),
    };
    assert_eq!(key, Bytes::from_static(b"\x01"));
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[Howned brief");
    assert_eq!(writes.recv().await.unwrap(), Bytes::from_static(b"\x0b"));
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");
    let error = response.await.unwrap().unwrap_err();
    assert_eq!(PromptNotShown::is(&error), !enter, "{error}");
    if enter {
        assert!(
            error.to_string().contains("remaining draft was cleared"),
            "{error}"
        );
    }
    assert!(
        writes.try_recv().is_err(),
        "no replay, second paste or late Enter"
    );
}

#[tokio::test]
async fn a_paste_ack_timeout_withdraws_the_visible_draft_without_enter() {
    failed_write_withdraws_owned_draft(false).await;
}

#[tokio::test]
async fn an_enter_ack_timeout_withdraws_the_owned_draft_and_preserves_uncertainty() {
    failed_write_withdraws_owned_draft(true).await;
}

async fn preserve_unrecognised_draft_on_withdrawal(ack_failed: bool) {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let release = ack_failed.then(|| clock.fail_next_write_completion(io::ErrorKind::TimedOut));
    let completion = runtime
        .queue_observed_input_submission(
            Bytes::from_static(b"owned brief"),
            Bytes::from_static(b"\r"),
            vec![
                Bytes::from_static(b"\x01"),
                Bytes::from_static(b"\x0b"),
                Bytes::from_static(b"\x15"),
            ],
            Duration::ZERO,
            None,
            |screen| {
                if screen.contains("developer edit") {
                    InputObservation::Other
                } else if screen.contains("owned brief") {
                    InputObservation::Pending
                } else {
                    InputObservation::Empty
                }
            },
        )
        .unwrap();
    assert_eq!(
        writes.recv().await.unwrap(),
        Bytes::from_static(b"owned brief")
    );
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[Howned brief + developer edit");
    if !ack_failed {
        clock.wait_for_draft_wait();
    }
    clock.advance(Duration::from_millis(2100));
    if let Some(release) = release {
        release.send(()).unwrap();
    }
    let mut response = tokio::task::spawn_blocking(move || {
        completion
            .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
            .unwrap()
    });
    let mut keys = Vec::new();
    let result = loop {
        tokio::select! {
            result=&mut response=>break result.unwrap(),
            key=writes.recv()=>{
                let key=key.unwrap();keys.push(key.clone());
                if key.as_ref()==b"\x0b" || key.as_ref()==b"\x15" {runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");}
                else {runtime.test_process_pty_bytes(b"\x1b[2J\x1b[Howned brief + developer edit");}
            }
        }
    };
    assert!(
        keys.is_empty(),
        "unowned draft received destructive keys: {keys:?}"
    );
    assert!(runtime
        .terminal
        .visible_ansi()
        .contains("owned brief + developer edit"));
    let error = result.unwrap_err();
    assert!(
        !PromptNotShown::is(&error),
        "changed input is not safe to replay: {error}"
    );
    assert!(error.to_string().contains("draft was left"), "{error}");
}

#[tokio::test]
async fn a_paste_ack_timeout_preserves_a_changed_unrecognised_draft() {
    preserve_unrecognised_draft_on_withdrawal(true).await;
}

#[tokio::test]
async fn a_render_timeout_preserves_a_changed_unrecognised_draft() {
    preserve_unrecognised_draft_on_withdrawal(false).await;
}

async fn stalled_cleanup_acknowledgement(hard_limit: Duration, assertion_window: Duration) {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    // Keep the real clock: a paused clock deliberately stretches channel waits
    // and cannot exercise an acknowledgement that never arrives.
    let reserve = withdrawal_budget(
        (0..64)
            .map(|_| "item")
            .collect::<Vec<_>>()
            .join("\n")
            .as_bytes(),
    );
    assert!(reserve > WITHDRAW_GRACE);
    runtime.test_process_pty_bytes(b"owned brief");
    let release = clock.fail_next_write_completion(io::ErrorKind::TimedOut);
    let hard_deadline = clock.now() + hard_limit;
    let submission = ObservedSubmission {
        terminal: runtime.terminal.clone(),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        inspect: Box::new(|_: &str| InputObservation::Pending),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
        deadline: std::cell::Cell::new(hard_deadline),
        hard_deadline,
        withdraw_grace: reserve,
        clock,
        diagnostics: Default::default(),
    };
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let result = submission
            .withdraw_pending(io::Error::new(io::ErrorKind::TimedOut, "unconfirmed Enter"));
        result_tx.send((Instant::now(), result)).unwrap();
    });
    assert_eq!(writes.recv().await.unwrap(), Bytes::from_static(b"\x01"));
    let outcome = result_rx.recv_timeout(assertion_window);
    // Tear down the held test I/O even on RED; no sleeping native thread leaks.
    release.send(()).unwrap();
    if outcome.is_err() {
        let _ = result_rx
            .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
            .unwrap();
    }
    writer.join().unwrap();
    let (finished, result) = outcome.expect("cleanup ack wait exceeded the no-progress/hard bound");
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(!PromptNotShown::is(&error), "Enter may have run: {error}");
    assert!(
        error.to_string().contains("could not be safely cleared"),
        "{error}"
    );
    assert!(
        !error.to_string().contains("remaining draft was cleared"),
        "{error}"
    );
    assert!(
        finished <= hard_deadline + Duration::from_millis(200),
        "cleanup exceeded its original hard deadline"
    );
    assert!(runtime.terminal.visible_ansi().contains("owned brief"));
    assert!(
        writes.try_recv().is_err(),
        "no further clear key, paste or Enter"
    );
}

#[tokio::test]
async fn a_stalled_cleanup_acknowledgement_obeys_the_no_progress_bound() {
    stalled_cleanup_acknowledgement(Duration::from_secs(10), Duration::from_millis(1400)).await;
}

#[tokio::test]
async fn a_stalled_cleanup_acknowledgement_also_honors_the_shorter_hard_deadline() {
    stalled_cleanup_acknowledgement(Duration::from_millis(150), Duration::from_millis(500)).await;
}
