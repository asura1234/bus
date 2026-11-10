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
