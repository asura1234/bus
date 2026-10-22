use super::*;

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
