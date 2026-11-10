//! A picker refusal must precede the durable editor reservation.
use super::*;
use bytes::Bytes;
use std::time::Duration;

#[tokio::test]
async fn codex_legacy_picker_does_not_reserve_a_draft_exchange() {
    let directory = crate::utils::test_temp::unique_temp_path("codex-picker-no-exchange");
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_process_pty_bytes("\x1b[2J\x1b[HMentions\r\nFiles, directories, plugins, skills and tasks\r\nno matches\r\nenter/tab insert · esc close · ↑/↓ select · ←/→ filter\r\n\r\n› developer draft @file\r\n\r\n GPT-6 · /repo".as_bytes());
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(b"from bus"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            None,
            Box::new(preserving_observer(
                AgentKind::Codex,
                "from bus".into(),
                directory.clone(),
                vec![
                    Bytes::from_static(b"\x13"),
                    Bytes::from_static(b"\x07"),
                    Bytes::from_static(b"\x7f"),
                ],
            )),
        )
        .unwrap();
    let error = completion
        .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
        .unwrap()
        .unwrap_err();
    assert!(crate::terminal::runtime::PromptNotShown::composer_busy(
        &error
    ));
    assert!(
        writes.try_recv().is_err(),
        "the developer's picker must receive no key"
    );
    let reserved = directory.join("request.json").exists();
    if directory.exists() {
        std::fs::remove_dir_all(&directory).unwrap();
    }
    assert!(
        !reserved,
        "blocked input reserved a durable editor handoff and prevents later delivery"
    );
}

#[tokio::test]
async fn codex_legacy_editor_return_with_mentions_restores_the_exact_draft() {
    const ORIGINAL: &str = "DEVELOPER_DRAFT_951 @UNMATCHED_FILE_951";
    const PAYLOAD: &str = "from bus";
    let root = crate::utils::test_temp::unique_temp_path("codex-editor-mentions");
    std::fs::create_dir_all(&root).unwrap();
    let directory = root.join("exchange");
    let file = root.join("native.txt");
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(120, 30, 16);
    runtime.test_submission_clock().unwrap().pause();
    let render = |text: &str, popup: bool| {
        let chrome = if popup {
            "Mentions\r\nFiles, directories, plugins, skills and tasks\r\nno matches\r\nenter/tab insert · esc close · ↑/↓ select · ←/→ filter\r\n\r\n"
        } else {
            ""
        };
        runtime.test_process_pty_bytes(
            format!("\x1b[2J\x1b[H{chrome}› {text}\r\n\r\n GPT-6 · /repo").as_bytes(),
        );
    };
    // The developer has already dismissed the original picker; an editor
    // return can independently reopen it while Bus still owns its marker.
    render(ORIGINAL, false);
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(PAYLOAD.as_bytes()),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            None,
            Box::new(preserving_observer(
                AgentKind::Codex,
                PAYLOAD.into(),
                directory.clone(),
                vec![
                    Bytes::from_static(b"\x13"),
                    Bytes::from_static(b"\x07"),
                    Bytes::from_static(b"\x7f"),
                ],
            )),
        )
        .unwrap();
    let mut completion = tokio::task::spawn_blocking(move || {
        completion
            .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
            .unwrap()
    });
    let mut draft = ORIGINAL.to_owned();
    let mut popup = false;
    let (mut editors, mut enters, mut deletions, mut escapes) = (0, 0, 0, 0);
    let result = loop {
        let key = tokio::select! {
            result = &mut completion => break result.unwrap(),
            key = writes.recv() => key.unwrap(),
        };
        match key.as_ref() {
            b"\x07" => {
                assert!(!popup, "the editor key cannot be sent into Mentions");
                editors += 1;
                std::fs::write(&file, &draft).unwrap();
                crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
                draft = std::fs::read_to_string(&file).unwrap();
                popup = draft.starts_with(ORIGINAL);
            }
            b"\x1b" => {
                assert!(
                    popup && draft.starts_with(ORIGINAL),
                    "only the restoration picker may be closed"
                );
                escapes += 1;
                popup = false;
            }
            b"\r" => {
                enters += 1;
                assert_eq!(enters, 1, "never replay a submission key");
                assert_eq!(draft, PAYLOAD, "never submit the developer draft");
                draft.clear();
                popup = false;
            }
            key if !key.is_empty() && key.iter().all(|byte| *byte == 0x7f) => {
                assert!(!popup, "attest and close the picker before marker deletion");
                let marker = draft.find("__BUS_DRAFT_END_").unwrap();
                assert_eq!(
                    key.len(),
                    draft.len() - marker,
                    "delete only the current editor marker"
                );
                draft.truncate(marker);
                deletions += 1;
                // A native edit can recompute Mentions after the first Esc.
                popup = draft == ORIGINAL;
            }
            other => panic!("unexpected paste or native key: {other:?}"),
        }
        render(&draft, popup);
    };
    let pending = directory.join("request.json").exists();
    let preserved = directory.join("preserved.json").exists();
    std::fs::remove_dir_all(&root).unwrap();
    result.unwrap();
    assert_eq!(
        draft, ORIGINAL,
        "a native editor return must not strand its marker in the developer draft"
    );
    assert!(!popup);
    assert_eq!((editors, enters, deletions, escapes), (3, 1, 3, 4));
    assert!(
        !pending && !preserved,
        "the verified restoration must release its durable reservation"
    );
    assert!(writes.try_recv().is_err());
}
