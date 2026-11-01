use super::*;

#[test]
fn codex_image_pastes_accept_each_chip_or_literal_path_before_the_reply_text() {
    let text = "[Bus message 3; answer with bus send --reply-to 3 --as 2 --text 'YOUR ANSWER']";
    let first = "\"/private/tmp/r211bad-h/image.png\"";
    let second = "\"/tmp/another image.png\"";
    for (paths, forms) in [
        (
            first.to_owned(),
            vec![first.to_owned(), "[Image #1] ".into()],
        ),
        (
            format!("{first}\n{second}"),
            vec![
                format!("{first}{second}"),
                format!("[Image #1] {second}"),
                format!("{first}[Image #1] "),
                "[Image #1] [Image #2] ".into(),
            ],
        ),
    ] {
        let expected = format!("{text}\n{paths}");
        for prefix in forms {
            for remaining in [
                text.to_owned(),
                format!("[Pasted Content {} chars]", text.chars().count()),
            ] {
                let screen = format!("› {prefix}{remaining}\n\n  gpt-5.4 default · /repo");
                assert_eq!(
                    observe(AgentKind::Codex, &screen, &expected),
                    InputObservation::Pending,
                    "{screen}"
                );
                for wrong in [
                    expected.replace("reply-to 3", "reply-to 30"),
                    format!("{expected}\n\"/tmp/missing.png\""),
                ] {
                    assert_eq!(
                        observe(AgentKind::Codex, &screen, &wrong),
                        InputObservation::Other,
                        "{wrong}"
                    );
                }
            }
        }
    }
    let expected = format!("{text}\n{first}\n{second}");
    for prefix in [
        "".to_owned(),
        first.to_owned(),
        format!("{second}{first}"),
        format!("[Image #1] {first}"),
        format!("{first}{second}{second}"),
        format!("[Image #1] {first}{second}"),
        "[Image #1] [Image #2] [Image #3] ".into(),
        "\"/tmp/unrelated.png\"".into(),
    ] {
        let screen = format!("› {prefix}{text}\n\n  gpt-5.4 default · /repo");
        assert_eq!(
            observe(AgentKind::Codex, &screen, &expected),
            InputObservation::Other,
            "{screen}"
        );
    }
}

#[tokio::test]
async fn codex_image_fallback_drafts_receive_exactly_one_enter() {
    let text = "[Bus message 3; answer with bus send --reply-to 3 --as 2 --text 'YOUR ANSWER']";
    let path = "\"/tmp/not-decodable.png\"";
    for (prefix, images) in [
        (path.to_owned(), path.to_owned()),
        (
            format!("[Image #1] {path}"),
            format!("\"/tmp/valid.png\"\n{path}"),
        ),
        (
            format!("{path}[Image #1] "),
            format!("{path}\n\"/tmp/valid.png\""),
        ),
    ] {
        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(160, 24, 32);
        let show = |body: &str| {
            runtime.test_process_pty_bytes(
                format!("\x1b[2J\x1b[H› {body}\r\n\r\n  gpt-5.4 default · /repo").as_bytes(),
            );
        };
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        runtime.test_submission_clock().unwrap().pause();
        show("");
        let completion = super::super::queue_prompt_input(
            &runtime,
            AgentKind::Codex,
            format!("{text}\n{images}"),
            true,
            std::time::Duration::ZERO,
            None,
            None,
            false,
        )
        .unwrap();
        let paste =
            tokio::time::timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT, writes.recv())
                .await
                .unwrap()
                .unwrap();
        assert!(paste.starts_with(b"\x1b[200~\"/tmp/"), "{paste:?}");
        assert!(
            paste.ends_with(b"\x1b[201~"),
            "Enter must follow attribution, not the paste"
        );
        show(&format!("{prefix}{text}"));
        let enter =
            tokio::time::timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT, writes.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(enter.as_ref(), b"\r");
        show("");
        assert!(completion
            .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
            .unwrap()
            .is_ok());
        assert!(
            writes.try_recv().is_err(),
            "one correlated draft must be submitted only once"
        );
    }
}
