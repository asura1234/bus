use super::*;

pub(crate) async fn submits_once_and_restores_existing_developer_draft(
    inspector: InspectorFactory,
) {
    const ORIGINAL: &str = "developer draft stays local";
    let directory = crate::utils::test_temp::unique_temp_path("codex-clipped-existing-draft");
    std::fs::create_dir_all(&directory).unwrap();
    let native_file = directory.join("native.txt");
    let exchange = directory.join("exchange");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(95, 44, 16);
    runtime.test_submission_clock().unwrap().pause();
    let original_frame = BEFORE.replace("› Ask Codex to do anything", &format!("› {ORIGINAL}"));
    assert_ne!(original_frame, BEFORE);
    render(&runtime, &original_frame);
    let paste = Bytes::from(format!("\x1b[200~{TEXT}\x1b[201~"));
    let completion = runtime
        .queue_observed_input_with_inspector(
            paste.clone(),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            None,
            inspector(exchange.clone()),
        )
        .unwrap();
    // Race native writes against the actual submission result. A nested
    // Exchange refusal must fail on its ComposerBusy error immediately,
    // rather than waiting for a verification key that will never arrive.
    let mut completion =
        tokio::task::spawn_blocking(move || completion.recv_timeout(NATIVE_TEST_TIMEOUT));
    let mut draft = ORIGINAL.to_owned();
    let (mut editors, mut pastes, mut enters, mut marker_deletions) = (0, 0, 0, 0);
    loop {
        let key = tokio::select! {
            result = &mut completion => {
                result
                    .expect("submission waiter failed")
                    .expect("submission did not finish")
                    .expect("clipped reply must submit while preserving the existing developer draft");
                break;
            }
            key = writes.recv() => key.expect("provider input channel closed"),
        };
        match key.as_ref() {
            b"\x07" => {
                editors += 1;
                std::fs::write(&native_file, &draft).unwrap();
                crate::platform::draft_editor::test_run_at(&exchange, &native_file).unwrap();
                draft = std::fs::read_to_string(&native_file).unwrap();
                if editors == 1 {
                    assert!(
                        draft.is_empty(),
                        "capture must retain and clear the developer draft"
                    );
                    render(&runtime, BEFORE);
                } else if let Some(marker) = draft.strip_prefix(TEXT) {
                    assert!(marker.starts_with("__BUS_DRAFT_END_"));
                    render(&runtime, CLIPPED);
                    // Keep this probe specific to nested exchange ownership;
                    // marker wrapping has its own real-provider acceptance.
                    runtime.test_process_pty_bytes(format!("\x1b[42;3H{marker}").as_bytes());
                } else {
                    assert!(draft.starts_with(ORIGINAL));
                    render(
                        &runtime,
                        &BEFORE.replace("› Ask Codex to do anything", &format!("› {draft}")),
                    );
                }
            }
            b"\r" => {
                enters += 1;
                assert_eq!(enters, 1, "the review reply must submit exactly once");
                assert_eq!(draft, TEXT, "never submit the preserved developer draft");
                draft.clear();
                render(&runtime, BEFORE);
            }
            bytes if !bytes.is_empty() && bytes.iter().all(|byte| *byte == 0x7f) => {
                marker_deletions += 1;
                let (body, marker) = draft.split_once("__BUS_DRAFT_END_").unwrap();
                assert_eq!(bytes.len(), "__BUS_DRAFT_END_".len() + marker.len());
                draft = body.to_owned();
                if draft == TEXT {
                    render(&runtime, CLIPPED);
                } else {
                    assert_eq!(
                        draft, ORIGINAL,
                        "delete only the editor acknowledgment marker"
                    );
                    render(&runtime, &original_frame);
                }
            }
            _ => {
                assert_eq!(key, paste, "unexpected native input");
                pastes += 1;
                assert_eq!(pastes, 1, "the review reply must be pasted exactly once");
                assert!(
                    draft.is_empty(),
                    "paste must follow exact developer-draft capture"
                );
                draft = TEXT.into();
                render(&runtime, CLIPPED);
            }
        }
    }
    assert_eq!((editors, pastes, enters, marker_deletions), (4, 1, 1, 3));
    assert_eq!(
        draft, ORIGINAL,
        "the complete developer draft must be restored"
    );
    assert!(
        writes.try_recv().is_err(),
        "no repeated paste, Enter, or cleanup"
    );
    assert!(!std::fs::read_dir(&exchange).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("draft-")
    }));
    std::fs::remove_dir_all(directory).unwrap();
}
