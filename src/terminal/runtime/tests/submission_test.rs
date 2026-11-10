use super::*;

#[path = "draft_preservation_test.rs"]
mod draft_preservation;

#[path = "fast_pane_submission_test.rs"]
mod fast_pane_submission;

#[path = "codex_clipped_composer_test.rs"]
pub(crate) mod codex_clipped_composer;

#[tokio::test]
async fn repeated_unowned_polls_emit_one_refusal_diagnostic_outside_the_render_lock() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Inspect {
        diagnosed: Arc<AtomicUsize>,
        content_lock: Arc<Mutex<()>>,
    }
    impl SubmissionInspector for Inspect {
        fn observe(&self, _: &str) -> InputObservation {
            InputObservation::Other
        }
        fn diagnose_refusal(&self, screen: &str, _: &'static str, _: Option<(u16, u16)>) {
            assert!(
                self.content_lock.try_lock().is_ok(),
                "diagnostic held the render lock"
            );
            assert!(screen.contains("unowned draft"));
            self.diagnosed.fetch_add(1, Ordering::SeqCst);
        }
    }
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_process_pty_bytes(b"unowned draft\r\n");
    let diagnosed = Arc::new(AtomicUsize::new(0));
    let expired = Instant::now() - Duration::from_secs(1);
    let submission = ObservedSubmission {
        scope: Default::default(),
        terminal: runtime.terminal.clone(),
        content_seq: runtime.content_seq.clone(),
        enter_content_seq: std::cell::Cell::new(0),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        inspect: Box::new(Inspect {
            diagnosed: diagnosed.clone(),
            content_lock: runtime.content_write_lock.clone(),
        }),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01")],
        deadline: std::cell::Cell::new(expired),
        hard_deadline: expired,
        withdraw_grace: WITHDRAW_GRACE,
        clock: SubmissionClock::default(),
        diagnostics: Default::default(),
        startup_suggestion_open: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    };
    for _ in 0..5 {
        assert_eq!(submission.observe("await_paste"), InputObservation::Other);
    }
    assert_eq!(
        diagnosed.load(Ordering::SeqCst),
        0,
        "polling emitted a refusal"
    );
    let error =
        submission.withdraw_before_enter(io::Error::new(io::ErrorKind::TimedOut, "expired"), None);
    assert!(error.is_err());
    submission.diagnose_refusal("result_error");
    assert_eq!(
        diagnosed.load(Ordering::SeqCst),
        1,
        "one refused attempt emitted duplicate diagnostics"
    );
    assert!(
        writes.try_recv().is_err(),
        "unowned draft received editing keys or Enter"
    );
}

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
        content_seq: runtime.content_seq.clone(),
        enter_content_seq: std::cell::Cell::new(0),
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
        startup_suggestion_open: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    };
    let result =
        submission.withdraw_before_enter(io::Error::new(io::ErrorKind::TimedOut, "expired"), None);
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
        content_seq: runtime.content_seq.clone(),
        enter_content_seq: std::cell::Cell::new(0),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        inspect: Box::new(|screen: &str| {
            if crate::terminal::driver::has_input_controls(screen) {
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
        startup_suggestion_open: runtime.startup_suggestion_open.clone(),
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
    assert!(
        runtime
            .startup_suggestion_open
            .load(std::sync::atomic::Ordering::Acquire),
        "a refused paste leaves the fresh-launch placeholder open"
    );
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
        content_seq: runtime.content_seq.clone(),
        enter_content_seq: std::cell::Cell::new(0),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        // Empty composer, our paste renders, Enter goes out, and the
        // provider accepts it; that last observation (render plus content
        // lock wait) finishes just after the confirmation deadline.
        inspect: Box::new(move |_: &str| {
            match seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0..=2 => InputObservation::Empty,
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
        clock: clock.clone(),
        diagnostics: Default::default(),
        startup_suggestion_open: runtime.startup_suggestion_open.clone(),
    };
    // Editor preservation still awaits consumption after its one guarded submit.
    let result = submission
        .run(Bytes::from_static(b"hello"), Duration::ZERO, None)
        .and_then(|()| submission.wait_for_consumption());
    assert!(
        clock.now() > deadline,
        "the acceptance observation crossed the deadline"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);
    assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"hello"));
    assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"\r"));
    assert!(writes.try_recv().is_err(), "one paste and one Enter only");
    assert!(
        result.is_ok(),
        "Enter was sent and the provider accepted the prompt: {result:?}"
    );
    assert!(
        !runtime
            .startup_suggestion_open
            .load(std::sync::atomic::Ordering::Acquire),
        "a successful prompt paste closes the fresh-launch placeholder"
    );
}

#[path = "submission_diagnostics_test.rs"]
mod diagnostics_tests;

#[path = "submission_write_failure_test.rs"]
mod write_failure_tests;

#[cfg(unix)]
#[path = "submission_backpressure_test.rs"]
mod backpressure_tests;

#[tokio::test]
async fn an_unfinished_synchronized_frame_is_never_given_to_an_ownership_inspector() {
    let (runtime, _) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_process_pty_bytes(b"stable draft");
    let now = Instant::now();
    let submission = ObservedSubmission {
        scope: Default::default(),
        terminal: runtime.terminal.clone(),
        content_seq: runtime.content_seq.clone(),
        enter_content_seq: std::cell::Cell::new(0),
        io: runtime.io.clone(),
        content_lock: runtime.content_write_lock.clone(),
        inspect: Box::new(|screen: &str| {
            assert!(
                !screen.contains("PARTIAL"),
                "an unfinished frame reached the ownership tracker"
            );
            InputObservation::Pending
        }),
        enter: Bytes::from_static(b"\r"),
        clear: vec![Bytes::from_static(b"\x01")],
        deadline: std::cell::Cell::new(now + Duration::from_secs(2)),
        hard_deadline: now + Duration::from_secs(3),
        withdraw_grace: WITHDRAW_GRACE,
        clock: SubmissionClock::default(),
        diagnostics: Default::default(),
        startup_suggestion_open: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    };
    assert_eq!(submission.observe("initial"), InputObservation::Pending);
    runtime.test_process_pty_bytes(b"\x1b[?2026h\x1b[2J\x1b[HPARTIAL");
    assert_eq!(
        submission.observe("late_paste"),
        InputObservation::Unavailable
    );
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[Hstable draft\x1b[?2026l");
    assert_eq!(submission.observe("late_paste"), InputObservation::Pending);
}

#[tokio::test]
async fn observed_queue_leaves_accept_and_writer_stamps_for_the_caller() {
    let (runtime, _writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
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
    let open = completion.submission_timestamps();
    assert_eq!(open.server_accepted_at_ms, None);
    assert_eq!(open.writer_queued_at_ms, None);
    completion.update_timestamps(|timestamps| {
        timestamps.server_accepted_at_ms.get_or_insert(1_000);
        timestamps.writer_queued_at_ms.get_or_insert(5_000);
    });
    let stamped = completion.submission_timestamps();
    assert_eq!(stamped.server_accepted_at_ms, Some(1_000));
    assert_eq!(stamped.writer_queued_at_ms, Some(5_000));
}
