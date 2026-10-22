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
        .recv_timeout(Duration::from_millis(100))
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
        terminal: runtime.terminal.clone(),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        inspect: Box::new(|_| InputObservation::Other),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01")],
        deadline: std::cell::Cell::new(expired),
        hard_deadline: expired,
    };
    let result = submission.withdraw_unshown(io::Error::new(io::ErrorKind::TimedOut, "expired"));
    assert!(result.is_err_and(|error| error.kind() == io::ErrorKind::TimedOut));
    assert!(
        writes.try_recv().is_err(),
        "withdrawal cannot restart its grace period"
    );
}

#[tokio::test]
async fn startup_controls_between_clear_and_paste_receive_no_message_bytes() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_process_pty_bytes("›\r\n\r\nGPT-6".as_bytes());
    let submission = ObservedSubmission {
        terminal: runtime.terminal.clone(),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        inspect: Box::new(|screen| {
            if crate::agents::dialog::parse(screen).is_some() {
                InputObservation::Unavailable
            } else {
                InputObservation::Empty
            }
        }),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01")],
        deadline: std::cell::Cell::new(Instant::now() + Duration::from_secs(2)),
        hard_deadline: Instant::now() + Duration::from_secs(3),
    };
    submission.clear_draft().unwrap();
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
    let deadline = Instant::now() + Duration::from_millis(500);
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    let submission = ObservedSubmission {
        terminal: runtime.terminal.clone(),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        // Empty composer, our paste renders, Enter goes out, and the
        // provider accepts it; that last observation (render plus content
        // lock wait) finishes just after the confirmation deadline.
        inspect: Box::new(
            move |_| match seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0..=2 => InputObservation::Empty,
                3 | 4 => InputObservation::Pending,
                _ => {
                    std::thread::sleep(
                        (deadline + Duration::from_millis(50))
                            .saturating_duration_since(Instant::now()),
                    );
                    InputObservation::Accepted
                }
            },
        ),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01")],
        deadline: std::cell::Cell::new(deadline),
        hard_deadline: deadline + Duration::from_secs(1),
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
