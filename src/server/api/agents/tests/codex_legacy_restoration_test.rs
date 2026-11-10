//! Failed legacy Enter must preserve the developer draft and remain uncertain.
use super::*;
use bytes::Bytes;
use std::{io, time::Duration};

const ORIGINAL: &str = "DEVELOPER_DRAFT_951 @UNMATCHED_FILE_951";
const PAYLOAD: &str = "from bus";

#[derive(Clone, Copy, PartialEq)]
enum Failure {
    Expired,
    Invalidated,
    Preserved,
    RestorationFailure,
}

async fn failed_editor_enter(failure: Failure) {
    let root = crate::utils::test_temp::unique_temp_path("codex-legacy-restore-failure");
    std::fs::create_dir_all(&root).unwrap();
    let directory = root.join("exchange");
    let file = root.join("native.txt");
    if matches!(failure, Failure::Preserved | Failure::RestorationFailure) {
        std::fs::write(&file, ORIGINAL).unwrap();
        let exchange = crate::platform::draft_editor::Exchange::begin_preserving(
            directory.clone(),
            Duration::from_secs(60),
        )
        .unwrap();
        exchange.request_swap(PAYLOAD).unwrap();
        crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
        // A prior swap installed this payload, then stopped before Enter.
        drop(exchange);
    }
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(120, 30, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
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
    let mut draft = if matches!(failure, Failure::Preserved | Failure::RestorationFailure) {
        PAYLOAD
    } else {
        ORIGINAL
    }
    .to_owned();
    render(&draft, false);
    let mut enter_ack = matches!(failure, Failure::Preserved | Failure::RestorationFailure)
        .then(|| clock.hold_next_write_completion());
    let mut restore_ack: Option<std::sync::mpsc::Sender<()>> = None;
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
    let (mut popup, mut editors, mut enters) = (false, 0, 0);
    let result = loop {
        let key = tokio::select! {
            result = &mut completion => break result.unwrap(),
            key = writes.recv() => key.unwrap(),
        };
        match key.as_ref() {
            b"\x07" => {
                assert!(!popup, "never open the editor inside a picker");
                editors += 1;
                if failure == Failure::RestorationFailure && enters > 0 {
                    // Consumption was seen, but this restoration key's
                    // receipt fails before an editor exchange can complete.
                    restore_ack.take().unwrap().send(()).unwrap();
                    continue;
                }
                std::fs::write(&file, &draft).unwrap();
                crate::platform::draft_editor::test_run_at(&directory, &file).unwrap();
                draft = std::fs::read_to_string(&file).unwrap();
                popup = draft.starts_with(ORIGINAL);
                if failure == Failure::Invalidated && editors == 2 {
                    // Enter physically reached Codex, but its receipt failed.
                    // Revoke the restoration lease before its next key.
                    runtime.invalidate_submission_ownership();
                    render(&draft, popup);
                    restore_ack.take().unwrap().send(()).unwrap();
                }
            }
            b"\r" => {
                enters += 1;
                assert_eq!(enters, 1, "never replay Enter");
                assert_eq!(draft, PAYLOAD, "never submit the developer draft");
                // The first three modes ignore Enter; the final mode consumes
                // it before a failed restoration and a later returned paste.
                if failure == Failure::RestorationFailure {
                    draft.clear();
                    restore_ack = Some(clock.fail_next_write_completion(io::ErrorKind::Other));
                } else if failure == Failure::Invalidated {
                    restore_ack = Some(clock.hold_next_write_completion());
                } else {
                    clock.advance(Duration::from_secs(3));
                }
                enter_ack.take().unwrap().send(()).unwrap();
            }
            b"\x1b" => {
                assert!(popup && draft.starts_with(ORIGINAL));
                popup = false;
            }
            b"\x01" => {}
            b"\x0b" => draft.clear(),
            bytes if !bytes.is_empty() && bytes.iter().all(|b| *b == 0x7f) => {
                assert!(!popup, "close attested picker before deleting its token");
                let marker = draft.find("__BUS_DRAFT_END_").unwrap();
                assert_eq!(bytes.len(), draft.len() - marker);
                draft.truncate(marker);
                popup = draft == ORIGINAL;
                if enters == 0 && draft == PAYLOAD {
                    enter_ack = Some(if failure == Failure::Invalidated {
                        clock.fail_next_write_completion(io::ErrorKind::Other)
                    } else {
                        clock.hold_next_write_completion()
                    });
                }
            }
            other => panic!("unexpected paste or native key: {other:?}"),
        }
        render(&draft, popup);
    };
    if failure == Failure::RestorationFailure {
        assert!(
            result.is_ok(),
            "consumption was confirmed before restoration failed"
        );
        // A transient empty frame did not transfer the Bus paste's ownership.
        // A later complete render must still be withdrawn without another Enter.
        render(PAYLOAD, false);
        let recovery = runtime
            .queue_observed_input_with_inspector(
                Bytes::from_static(b"next request"),
                Bytes::from_static(b"\r"),
                vec![],
                Duration::ZERO,
                None,
                Box::new(|_: &str| InputObservation::Unavailable),
            )
            .unwrap();
        let mut recovery = tokio::task::spawn_blocking(move || {
            recovery
                .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
                .unwrap()
        });
        let mut cleared = false;
        let error = loop {
            tokio::select! {
                result = &mut recovery => break result.unwrap().unwrap_err(),
                key = writes.recv() => {
                    let key = key.unwrap();
                    assert!(matches!(key.as_ref(), b"\x01" | b"\x0b"), "no old Enter/new paste: {key:?}");
                    if key.as_ref() == b"\x0b" { cleared = true; }
                    render(if cleared { "" } else { PAYLOAD }, false);
                }
            }
        };
        assert!(
            cleared,
            "the preserved editor path lost ownership of returned Bus text: {error}"
        );
    }
    let preserved = crate::platform::draft_editor::Exchange::has_preserved_draft(&directory);
    let saved_original = directory.read_dir().unwrap().any(|entry| {
        let path = entry.unwrap().path();
        path.extension().is_some_and(|ext| ext == "txt")
            && std::fs::read_to_string(path).is_ok_and(|text| text == ORIGINAL)
    });
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(enters, 1);
    if failure == Failure::RestorationFailure {
        assert!(
            preserved && saved_original,
            "failed restoration retains the exact developer backup"
        );
    } else if failure == Failure::Invalidated {
        let error = result.expect_err("a possibly-run Enter must stay uncertain");
        assert!(
            !crate::terminal::runtime::PromptNotShown::is(&error),
            "restoration refusal must not authorize a second submission: {error}"
        );
        assert!(
            preserved && saved_original,
            "revoked cleanup retains the exact developer backup"
        );
    } else {
        assert_eq!(
            draft, ORIGINAL,
            "ignored Enter must not strand Bus input or a restoration marker"
        );
        assert!(
            !popup && !preserved,
            "verified restoration releases the exchange"
        );
        let error = result.expect_err("writer receipt alone is not consumption");
        assert!(
            !crate::terminal::runtime::PromptNotShown::is(&error),
            "{error}"
        );
    }
    assert!(
        writes.try_recv().is_err(),
        "no additional Enter or editing key"
    );
}

#[tokio::test]
async fn codex_legacy_expired_enter_restores_mentions_draft() {
    failed_editor_enter(Failure::Expired).await;
}

#[tokio::test]
async fn codex_legacy_post_enter_restoration_refusal_is_uncertain() {
    failed_editor_enter(Failure::Invalidated).await;
}

#[tokio::test]
async fn codex_legacy_preserved_payload_ignored_enter_restores_draft() {
    failed_editor_enter(Failure::Preserved).await;
}

#[tokio::test]
async fn codex_legacy_preserved_editor_retains_late_input_after_restore_failure() {
    failed_editor_enter(Failure::RestorationFailure).await;
}
