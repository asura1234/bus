use super::*;

#[tokio::test]
async fn a_delayed_start_reserves_withdrawal_time_before_typing() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    // Dispatch has used the confirmation allowance; there is only withdrawal
    // time left. No new paste may consume that reserve.
    let completion = runtime
        .queue_observed_input_submission(
            Bytes::from_static(b"hello"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01")],
            Duration::ZERO,
            Some(Instant::now() + Duration::from_millis(500)),
            |_| InputObservation::Empty,
        )
        .unwrap();
    let error = completion
        .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
        .expect("no confirmation time remains for a new paste")
        .unwrap_err();
    assert!(
        PromptNotShown::is(&error),
        "definite pre-paste rejection: {error}"
    );
    assert!(
        writes.try_recv().is_err(),
        "withdrawal reserve must not be spent on a new paste"
    );
}

#[tokio::test]
async fn withdrawal_cannot_extend_the_hard_deadline_or_send_late_clear_keys() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let expired = Instant::now() - Duration::from_secs(1);
    let submission = ObservedSubmission {
        scope: Default::default(),
        terminal: runtime.terminal.clone(),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        inspect: Box::new(|_: &str| InputObservation::Pending),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01")],
        deadline: std::cell::Cell::new(expired),
        hard_deadline: expired,
        withdraw_grace: Duration::from_secs(10),
        clock: SubmissionClock::default(),
        diagnostics: Default::default(),
    };
    let result =
        submission.withdraw_before_enter(io::Error::new(io::ErrorKind::TimedOut, "expired"));
    assert!(result.is_err_and(|error| error.kind() == io::ErrorKind::TimedOut));
    assert!(
        writes.try_recv().is_err(),
        "withdrawal cannot restart its grace period"
    );
}

#[tokio::test]
async fn startup_controls_between_initial_observation_and_paste_receive_no_message_bytes() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_process_pty_bytes("›\r\n\r\nGPT-6".as_bytes());
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let submission = ObservedSubmission {
        scope: Default::default(),
        terminal: runtime.terminal.clone(),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        inspect: Box::new(|screen: &str| {
            if crate::agents::dialog::parse(screen).is_some() {
                InputObservation::Unavailable
            } else {
                InputObservation::Empty
            }
        }),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01")],
        deadline: std::cell::Cell::new(clock.now() + Duration::from_secs(2)),
        hard_deadline: clock.now() + Duration::from_secs(3),
        withdraw_grace: WITHDRAW_GRACE,
        clock,
        diagnostics: Default::default(),
    };
    assert_eq!(submission.observe("initial"), InputObservation::Empty);
    runtime.test_process_pty_bytes(
        concat!(
            "\x1b[2J\x1b[HHooks need review\r\n",
            "› 1. Review hooks\r\n2. Trust all and continue\r\n",
            "3. Continue without trusting (hooks won't run)\r\n",
            "Press enter to confirm or esc to go back\r\n",
        )
        .as_bytes(),
    );
    let result = submission.write(Bytes::from_static(b"first message"), Duration::ZERO);
    assert!(writes.try_recv().is_err(), "chooser received message bytes");
    assert!(result.is_err_and(|error| PromptNotShown::is(&error)));
}

#[tokio::test]
async fn an_acceptance_observed_as_the_deadline_passes_still_counts_as_delivered() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    let deadline = clock.now() + crate::terminal::runtime::NATIVE_TEST_TIMEOUT;
    let observed_clock = clock.clone();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    let submission = ObservedSubmission {
        scope: Default::default(),
        terminal: runtime.terminal.clone(),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        // Empty composer, our paste renders, Enter goes out, and the
        // provider accepts it; that last observation (render plus content
        // lock wait) finishes just after the confirmation deadline.
        inspect: Box::new(move |_: &str| {
            match seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0..=2 => InputObservation::Empty,
                3 | 4 => InputObservation::Pending,
                _ => {
                    observed_clock.pause();
                    observed_clock.advance(
                        (deadline + Duration::from_millis(50))
                            .saturating_duration_since(observed_clock.now()),
                    );
                    InputObservation::Accepted
                }
            }
        }),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01")],
        deadline: std::cell::Cell::new(deadline),
        hard_deadline: deadline + Duration::from_secs(1),
        withdraw_grace: WITHDRAW_GRACE,
        clock,
        diagnostics: Default::default(),
    };
    let result = submission.run(Bytes::from_static(b"hello"), Duration::ZERO);
    assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"hello"));
    assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"\r"));
    assert!(writes.try_recv().is_err(), "one paste and one Enter only");
    assert!(
        result.is_ok(),
        "Enter was sent and the provider accepted the prompt: {result:?}"
    );
}

#[tokio::test]
async fn an_owned_draft_edited_before_first_enter_is_preserved() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let (seen_tx, seen_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let first_owned = AtomicBool::new(true);
    let completion = runtime
        .queue_observed_input_submission(
            Bytes::from_static(b"owned draft"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            None,
            move |screen| {
                if screen.contains("owned draft + developer edit") {
                    InputObservation::Other
                } else if screen.contains("owned draft") {
                    if first_owned.swap(false, Ordering::SeqCst) {
                        seen_tx.send(()).unwrap();
                        resume_rx
                            .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
                            .unwrap();
                    }
                    InputObservation::Pending
                } else {
                    InputObservation::Empty
                }
            },
        )
        .unwrap();
    assert_eq!(
        writes.recv().await.unwrap(),
        Bytes::from_static(b"owned draft")
    );
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[Howned draft");
    seen_rx
        .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
        .unwrap();
    // Ownership was established; a developer edits before the first Enter.
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[Howned draft + developer edit");
    clock.advance(Duration::from_millis(2100));
    resume_tx.send(()).unwrap();
    let mut result = tokio::task::spawn_blocking(move || {
        completion
            .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
            .unwrap()
    });
    let (error, destructive) = tokio::select! {
        result = &mut result => (result.unwrap().unwrap_err(), None),
        key = writes.recv() => {
            // Let the bad implementation terminate before asserting its write.
            clock.advance(Duration::from_secs(2));
            (result.await.unwrap().unwrap_err(), key)
        }
    };
    assert_eq!(
        destructive, None,
        "cleanup must not edit input after ownership is lost"
    );
    assert!(
        !PromptNotShown::is(&error),
        "changed input is unsafe to replay: {error}"
    );
    for detail in ["draft was left", "edited", "transcript", "before resending"] {
        assert!(
            error.to_string().contains(detail),
            "missing {detail}: {error}"
        );
    }
    assert!(runtime
        .terminal
        .visible_ansi()
        .contains("owned draft + developer edit"));
    assert!(
        writes.try_recv().is_err(),
        "no Enter or destructive clear keys"
    );
}

#[path = "submission_diagnostics_test.rs"]
mod diagnostics_tests;

#[path = "submission_write_failure_test.rs"]
mod write_failure_tests;

#[cfg(unix)]
#[path = "submission_backpressure_test.rs"]
mod backpressure_tests;
