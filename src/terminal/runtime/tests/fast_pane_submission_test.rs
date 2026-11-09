use super::*;
use std::path::PathBuf;

const PAYLOAD: &[u8] = b"BUS_PAYLOAD";

#[test]
fn fast_pane_removes_pacing_without_spending_the_recovery_budget() {
    use crate::protocol::api::submission::{confirmation_budget, submit_delay};
    // A normal native writer needs no sleep, even for a multiline paste.
    assert_eq!(submit_delay(false, 823), Duration::ZERO);
    #[cfg(not(windows))]
    assert_eq!(submit_delay(true, 823), Duration::ZERO);
    // Removing pacing must not shorten the independent deadline (spike B).
    assert_eq!(
        confirmation_budget(Duration::ZERO),
        Duration::from_millis(2300)
    );
    #[cfg(windows)]
    assert_eq!(submit_delay(true, 823), Duration::from_millis(805));
}

fn render(runtime: &TerminalRuntime, text: &str) {
    runtime
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{}", text.replace('\n', "\r\n")).as_bytes());
}

async fn next(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    tokio::time::timeout(Duration::from_secs(5), writes.recv())
        .await
        .expect("provider did not receive the next transaction step")
        .unwrap()
}

#[tokio::test]
async fn fast_pane_enter_does_not_wait_for_bus_text_ownership() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    render(&runtime, "EMPTY");
    let completion = runtime
        .queue_observed_input_submission(
            Bytes::from_static(PAYLOAD),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01")],
            Duration::ZERO,
            None,
            |screen| {
                if screen.contains("EMPTY") {
                    InputObservation::Empty
                } else {
                    // The native box is present but its rendered text cannot
                    // attest our complete paste (e.g. the #294 clipped draft).
                    InputObservation::Other
                }
            },
        )
        .unwrap();
    assert_eq!(next(&mut writes).await.as_ref(), PAYLOAD);
    render(&runtime, "clipped native composer suffix");
    assert_eq!(next(&mut writes).await.as_ref(), b"\r");
    render(&runtime, "EMPTY");
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(writes.try_recv().is_err(), "one paste and one Enter only");
}

#[tokio::test]
async fn fast_pane_dialog_appearing_during_paste_receives_no_enter() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    render(&runtime, "EMPTY");
    let release = runtime
        .test_submission_clock()
        .unwrap()
        .hold_next_write_completion();
    let completion = runtime
        .queue_observed_input_submission(
            Bytes::from_static(PAYLOAD),
            Bytes::from_static(b"\r"),
            vec![],
            Duration::ZERO,
            None,
            |screen| {
                if screen.contains("EMPTY") {
                    InputObservation::Empty
                } else {
                    InputObservation::Unavailable
                }
            },
        )
        .unwrap();
    assert_eq!(next(&mut writes).await.as_ref(), PAYLOAD);
    render(
        &runtime,
        "Do you want to proceed?\n❯ 1. Yes\n  2. No\nEnter to confirm",
    );
    release.send(()).unwrap();
    assert!(completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .is_err());
    assert!(writes.try_recv().is_err(), "a dialog must receive no Enter");
}

#[tokio::test]
async fn fast_pane_draft_lease_buffers_immediately_after_a_developer_key() {
    use crate::terminal::runtime::draft_input::DraftInput;
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"x"))
        .unwrap();
    assert_eq!(next(&mut writes).await.as_ref(), b"x");
    let lease = DraftInput::acquire(&runtime.draft_input, &runtime.io)
        .expect("a developer draft must not require a three-second quiet window");
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"y"))
        .unwrap();
    assert!(writes.try_recv().is_err(), "new keys wait for restoration");
    lease.restored();
    drop(lease);
    assert_eq!(next(&mut writes).await.as_ref(), b"y");
}

struct Editor {
    directory: PathBuf,
}

impl SubmissionInspector for Editor {
    fn observe(&self, screen: &str) -> InputObservation {
        if screen.contains("EMPTY") {
            InputObservation::Empty
        } else if screen.contains("BUS_PAYLOAD") {
            InputObservation::Pending
        } else {
            InputObservation::Other
        }
    }
    fn draft_preservation(&self, _: &str) -> Option<DraftPreservation> {
        Some(DraftPreservation::Editor {
            key: Bytes::from_static(b"\x07"),
            backspace: Bytes::from_static(b"\x7f"),
            directory: self.directory.clone(),
        })
    }
    fn editor_submission(&self) -> Option<EditorSubmission> {
        Some(EditorSubmission {
            text: String::from_utf8(PAYLOAD.to_vec()).unwrap(),
            directory: self.directory.clone(),
            key: Bytes::from_static(b"\x07"),
            backspace: Bytes::from_static(b"\x7f"),
        })
    }
    fn restoration_marker_visible(&self, screen: &str, marker: &str) -> bool {
        screen.contains(marker)
    }
    fn preservation_budget(&self) -> Duration {
        Duration::from_secs(20)
    }
}

#[tokio::test]
async fn fast_editor_swap_captures_before_replacement_and_restores_exact_drafts() {
    for original in [
        String::new(),
        "\t  中文\n  second line\n\ntrailing \t \n\n".to_owned(),
        format!("{}\nend \t \n", "  long line 中文\n".repeat(800)),
    ] {
        let root = crate::utils::test_temp::unique_temp_path("fast-editor-swap");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("native-editor.txt");
        let directory = root.join("exchange");
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 32);
        let mut draft = original.clone();
        render(&runtime, if draft.is_empty() { "EMPTY" } else { "DRAFT" });
        let completion = runtime
            .queue_observed_input_with_inspector(
                Bytes::from_static(PAYLOAD),
                Bytes::from_static(b"\r"),
                vec![],
                Duration::ZERO,
                None,
                Box::new(Editor {
                    directory: directory.clone(),
                }),
            )
            .unwrap();
        let mut editors = 0;
        let mut enters = 0;
        let mut markers_removed = 0;
        loop {
            let key = next(&mut writes).await;
            match key.as_ref() {
                b"\x07" => {
                    editors += 1;
                    std::fs::write(&file, &draft).unwrap();
                    crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
                    let returned = std::fs::read_to_string(&file).unwrap();
                    if editors == 1 {
                        assert!(returned.starts_with("BUS_PAYLOAD__BUS_DRAFT_END_"),
                            "first handoff must save the old draft and install Bus input: {returned:?}");
                    }
                    draft = returned.trim_end().to_owned();
                    render(&runtime, if draft.is_empty() { "EMPTY" } else { &draft });
                }
                b"\r" => {
                    enters += 1;
                    assert_eq!(
                        draft.as_bytes(),
                        PAYLOAD,
                        "submit only the full Bus message"
                    );
                    draft.clear();
                    render(&runtime, "EMPTY");
                }
                bytes if !bytes.is_empty() && bytes.iter().all(|b| *b == 0x7f) => {
                    markers_removed += 1;
                    for _ in bytes {
                        assert!(draft.pop().unwrap().is_ascii());
                    }
                    render(&runtime, if draft.is_empty() { "EMPTY" } else { &draft });
                    if markers_removed == 3 {
                        break;
                    }
                }
                bytes => panic!("unexpected paste or transaction input: {bytes:?}"),
            }
        }
        completion
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(
            (editors, enters),
            (3, 1),
            "swap, restore, verify; never read back Bus ownership"
        );
        assert_eq!(draft.as_bytes(), original.as_bytes());
        assert!(
            !std::fs::read_dir(&directory)
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().starts_with("draft-")),
            "developer draft must never authorize automatic Bus-paste withdrawal"
        );
        assert!(writes.try_recv().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn existing_native_stash_restores_before_buffered_developer_keys_are_replayed() {
    struct ExistingStash {
        inspected: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }
    impl SubmissionInspector for ExistingStash {
        fn observe(&self, screen: &str) -> InputObservation {
            if screen.contains("EMPTY") {
                InputObservation::Empty
            } else {
                InputObservation::Other
            }
        }
        fn native_stash_visible(&self, screen: &str) -> bool {
            screen.contains("STASH")
        }
        fn native_draft_restored(&self, screen: &str) -> bool {
            if let Some(signal) = self.inspected.lock().unwrap().take() {
                let _ = signal.send(());
            }
            screen.contains("RESTORED")
        }
        fn preservation_budget(&self) -> Duration {
            Duration::from_secs(20)
        }
    }
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    let paste_receipt = clock.hold_next_write_completion();
    let (inspected, receipt) = tokio::sync::oneshot::channel();
    render(&runtime, "EMPTY STASH");
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(PAYLOAD),
            Bytes::from_static(b"\r"),
            vec![],
            Duration::ZERO,
            None,
            Box::new(ExistingStash {
                inspected: std::sync::Mutex::new(Some(inspected)),
            }),
        )
        .unwrap();
    assert_eq!(next(&mut writes).await.as_ref(), PAYLOAD);
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"developer key"))
        .unwrap();
    assert!(
        writes.try_recv().is_err(),
        "developer key is buffered during delivery"
    );
    render(&runtime, "BUS_PAYLOAD STASH");
    let enter_receipt = clock.hold_next_write_completion();
    paste_receipt.send(()).unwrap();
    assert_eq!(next(&mut writes).await.as_ref(), b"\r");
    enter_receipt.send(()).unwrap();
    tokio::select! {
        inspected = receipt => inspected.expect("native restoration receipt was abandoned"),
        key = writes.recv() => panic!("developer key replayed before native stash restoration: {key:?}"),
    }
    assert!(
        writes.try_recv().is_err(),
        "restoration has not happened yet"
    );
    render(&runtime, "RESTORED");
    assert_eq!(next(&mut writes).await.as_ref(), b"developer key");
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(
        writes.try_recv().is_err(),
        "never submit the restored native draft"
    );
}

#[tokio::test]
async fn consumed_editor_submit_stays_ok_when_restoration_does_not_finish() {
    struct FastEditor {
        directory: PathBuf,
    }
    impl SubmissionInspector for FastEditor {
        fn observe(&self, screen: &str) -> InputObservation {
            if screen.contains("EMPTY") {
                InputObservation::Empty
            } else if screen.contains("BUS_PAYLOAD") {
                InputObservation::Pending
            } else {
                InputObservation::Other
            }
        }
        fn draft_preservation(&self, _: &str) -> Option<DraftPreservation> {
            Some(DraftPreservation::Editor {
                key: Bytes::from_static(b"\x07"),
                backspace: Bytes::from_static(b"\x7f"),
                directory: self.directory.clone(),
            })
        }
        fn editor_submission(&self) -> Option<EditorSubmission> {
            Some(EditorSubmission {
                text: String::from_utf8(PAYLOAD.to_vec()).unwrap(),
                directory: self.directory.clone(),
                key: Bytes::from_static(b"\x07"),
                backspace: Bytes::from_static(b"\x7f"),
            })
        }
        fn restoration_marker_visible(&self, screen: &str, marker: &str) -> bool {
            screen.contains(marker)
        }
        fn preservation_budget(&self) -> Duration {
            Duration::from_secs(3)
        }
    }

    let root = crate::utils::test_temp::unique_temp_path("consumed-submit-restore-stall");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("native-editor.txt");
    let directory = root.join("exchange");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 32);
    let mut draft = String::from("saved developer draft");
    render(&runtime, "DRAFT");
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(PAYLOAD),
            Bytes::from_static(b"\r"),
            vec![],
            Duration::ZERO,
            None,
            Box::new(FastEditor {
                directory: directory.clone(),
            }),
        )
        .unwrap();
    let mut enters = 0;
    loop {
        let key = next(&mut writes).await;
        match key.as_ref() {
            b"\x07" => {
                if enters == 0 {
                    std::fs::write(&file, &draft).unwrap();
                    crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
                    draft = std::fs::read_to_string(&file)
                        .unwrap()
                        .trim_end()
                        .to_owned();
                    render(&runtime, if draft.is_empty() { "EMPTY" } else { &draft });
                }
            }
            b"\r" => {
                enters += 1;
                draft.clear();
                render(&runtime, "EMPTY");
                break;
            }
            bytes if !bytes.is_empty() && bytes.iter().all(|b| *b == 0x7f) => {
                for _ in bytes {
                    draft.pop();
                }
                render(&runtime, if draft.is_empty() { "EMPTY" } else { &draft });
            }
            bytes => panic!("unexpected input before submit: {bytes:?}"),
        }
    }
    let outcome = completion.recv_timeout(Duration::from_secs(8)).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(enters, 1);
    assert!(
        outcome.is_ok(),
        "Enter was sent and the composer was empty, so a later restoration failure must not fail the submit: {outcome:?}"
    );
}

#[tokio::test]
async fn occupied_editor_with_a_preserved_draft_does_not_send_editor_keys() {
    let root = crate::utils::test_temp::unique_temp_path("occupied-preserved-draft");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("native-editor.txt");
    let directory = root.join("exchange");
    std::fs::write(&file, "original draft").unwrap();
    {
        let exchange = crate::platform::draft_editor::Exchange::begin_preserving(
            directory.clone(),
            Duration::from_secs(60),
        )
        .unwrap();
        exchange.request_swap("attempt").unwrap();
        crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
    }
    assert!(crate::platform::draft_editor::Exchange::has_preserved_draft(&directory));
    let before = std::fs::read_to_string(&file).unwrap();
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 32);
    render(&runtime, "NEW DRAFT");
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(PAYLOAD),
            Bytes::from_static(b"\r"),
            vec![],
            Duration::ZERO,
            None,
            Box::new(Editor {
                directory: directory.clone(),
            }),
        )
        .unwrap();
    let outcome = tokio::select! {
        key = writes.recv() => panic!("occupied editor must defer without an editor key, got {key:?}"),
        outcome = async {
            tokio::task::spawn_blocking(move || completion.recv_timeout(Duration::from_secs(5)))
                .await
                .unwrap()
        } => outcome.unwrap(),
    };
    let after = std::fs::read_to_string(&file).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(after, before, "the live editor file must stay untouched");
    let error = outcome.expect_err("an occupied preserved editor defers");
    assert!(
        crate::terminal::runtime::PromptNotShown::composer_busy(&error),
        "{error}"
    );
}

#[tokio::test]
async fn pending_payload_with_a_preserved_draft_still_submits_once() {
    let root = crate::utils::test_temp::unique_temp_path("pending-preserved-submit");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("native-editor.txt");
    let directory = root.join("exchange");
    std::fs::write(&file, "original draft").unwrap();
    {
        let exchange = crate::platform::draft_editor::Exchange::begin_preserving(
            directory.clone(),
            Duration::from_secs(60),
        )
        .unwrap();
        exchange.request_swap("attempt").unwrap();
        crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
    }
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 32);
    render(&runtime, "BUS_PAYLOAD");
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(PAYLOAD),
            Bytes::from_static(b"\r"),
            vec![],
            Duration::ZERO,
            None,
            Box::new(Editor {
                directory: directory.clone(),
            }),
        )
        .unwrap();
    let mut completion_task =
        tokio::task::spawn_blocking(move || completion.recv_timeout(Duration::from_secs(5)));
    let mut enters = 0;
    let outcome = loop {
        tokio::select! {
            key = writes.recv() => {
                match key.as_deref() {
                    Some(b"\r") => enters += 1,
                    Some(other) => panic!("pending payload must submit, got {other:?}"),
                    None => panic!("input channel closed before the submit finished"),
                }
            }
            outcome = &mut completion_task => break outcome.unwrap().unwrap(),
        }
    };
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(
        enters, 1,
        "the preserved attempt is already on screen and must submit once"
    );
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[tokio::test]
async fn empty_composer_restore_failure_stays_retryable() {
    struct FastEditor {
        directory: PathBuf,
    }
    impl SubmissionInspector for FastEditor {
        fn observe(&self, screen: &str) -> InputObservation {
            if screen.contains("EMPTY") {
                InputObservation::Empty
            } else if screen.contains("BUS_PAYLOAD") {
                InputObservation::Pending
            } else {
                InputObservation::Other
            }
        }
        fn draft_preservation(&self, _: &str) -> Option<DraftPreservation> {
            Some(DraftPreservation::Editor {
                key: Bytes::from_static(b"\x07"),
                backspace: Bytes::from_static(b"\x7f"),
                directory: self.directory.clone(),
            })
        }
        fn editor_submission(&self) -> Option<EditorSubmission> {
            Some(EditorSubmission {
                text: String::from_utf8(PAYLOAD.to_vec()).unwrap(),
                directory: self.directory.clone(),
                key: Bytes::from_static(b"\x07"),
                backspace: Bytes::from_static(b"\x7f"),
            })
        }
        fn restoration_marker_visible(&self, screen: &str, marker: &str) -> bool {
            screen.contains(marker)
        }
        fn preservation_budget(&self) -> Duration {
            Duration::from_secs(3)
        }
    }

    let root = crate::utils::test_temp::unique_temp_path("empty-restore-failure");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("native-editor.txt");
    let directory = root.join("exchange");
    std::fs::write(&file, "original draft").unwrap();
    {
        let exchange = crate::platform::draft_editor::Exchange::begin_preserving(
            directory.clone(),
            Duration::from_secs(60),
        )
        .unwrap();
        exchange.request_swap("attempt").unwrap();
        crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
    }
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 32);
    render(&runtime, "EMPTY");
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(PAYLOAD),
            Bytes::from_static(b"\r"),
            vec![],
            Duration::ZERO,
            None,
            Box::new(FastEditor {
                directory: directory.clone(),
            }),
        )
        .unwrap();
    let mut completion_task =
        tokio::task::spawn_blocking(move || completion.recv_timeout(Duration::from_secs(8)));
    let mut enters = 0;
    let outcome = loop {
        tokio::select! {
            biased;
            key = writes.recv() => {
                let key = key.expect("input channel closed");
                if key.as_ref() == b"\r" {
                    enters += 1;
                }
            }
            outcome = &mut completion_task => break outcome.unwrap().unwrap(),
        }
    };
    assert_eq!(enters, 0, "a failed restore must not submit");
    let error = outcome.expect_err("a failed restore defers the submit");
    assert!(
        crate::terminal::runtime::PromptNotShown::composer_busy(&error),
        "{error}"
    );
    assert!(crate::platform::draft_editor::Exchange::has_preserved_draft(&directory));
    std::fs::remove_dir_all(root).unwrap();
}
