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
