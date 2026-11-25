use super::{paint, Duration, PreservingComposer};
use crate::{
    agents::AgentKind,
    devtools::tui::FakeClaudeComposer,
    terminal::runtime::{PromptNotShown, TerminalRuntime},
};
use bytes::Bytes;

const HIDDEN: &str = "fake:draft-bytes HIDDEN";
const VISIBLE: &str = "fake:draft-bytes VISIBLE";
const MESSAGE: &str = "STASH full native slot";

struct Transfer {
    fake: FakeClaudeComposer,
    result: std::io::Result<()>,
    submitted: Vec<String>,
    keys: Vec<Bytes>,
}

async fn transfer(hidden: Option<&str>, visible: &str, delay_stash: bool) -> Transfer {
    let mut fake = FakeClaudeComposer::new();
    if let Some(hidden) = hidden {
        assert!(fake.input(hidden.as_bytes()).is_none());
        assert!(fake.input(b"\x13").is_none());
        assert_eq!(fake.stashed(), Some(hidden));
    }
    assert!(fake.input(visible.as_bytes()).is_none());
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    paint(&runtime, &fake.shown(), fake.stashed().is_some(), "");
    let directory = crate::utils::test_temp::unique_temp_path("claude-native-stash");
    if delay_stash {
        runtime.test_set_input_recovery_directory(directory.join("held-input"));
    }
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(MESSAGE.as_bytes()),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x15")],
            Duration::ZERO,
            None,
            Box::new(PreservingComposer::new(
                AgentKind::Claude,
                MESSAGE.into(),
                directory.clone(),
                vec![
                    Bytes::from_static(b"\x13"),
                    Bytes::from_static(b"\x07"),
                    Bytes::from_static(b"\x7f"),
                ],
            )),
        )
        .unwrap();
    let mut done = tokio::task::spawn_blocking(move || {
        completion.recv_timeout(Duration::from_secs(12)).unwrap()
    });
    let mut submitted = Vec::new();
    let mut keys = Vec::new();
    let result = loop {
        tokio::select! {
            biased;
            Some(bytes) = writes.recv() => {
                if delay_stash && keys.is_empty() {
                    assert_eq!(bytes.as_ref(), b"\x13");
                    runtime.try_send_developer_bytes(Bytes::from_static(b"-2")).unwrap();
                    tokio::time::sleep(Duration::from_millis(558)).await;
                }
                if let Some(text) = fake.input(&bytes) {
                    submitted.push(text);
                }
                let transcript = submitted.last().map_or_else(String::new, |text| format!("❯ {text}"));
                paint(&runtime, &fake.shown().replace('\n', "\n  "), fake.stashed().is_some(), &transcript);
                keys.push(bytes);
            }
            result = &mut done => break result.unwrap(),
        }
    };
    if directory.exists() {
        std::fs::remove_dir_all(directory).unwrap();
    }
    Transfer {
        fake,
        result,
        submitted,
        keys,
    }
}

#[tokio::test]
async fn claude_occupied_stash_defers_without_touching_either_draft() {
    let mut transfer = transfer(Some(HIDDEN), VISIBLE, false).await;
    assert!(PromptNotShown::composer_busy(&transfer.result.unwrap_err()));
    assert!(transfer.submitted.is_empty());
    assert!(transfer.keys.is_empty());
    assert_eq!(transfer.fake.body(), VISIBLE);
    assert_eq!(transfer.fake.stashed(), Some(HIDDEN));
    assert_eq!(transfer.fake.input(b"\r").as_deref(), Some(VISIBLE));
    assert_eq!(transfer.fake.body(), HIDDEN);
    assert_eq!(transfer.fake.stashed(), None);
    assert_eq!(transfer.fake.input(b"\r").as_deref(), Some(HIDDEN));
}

#[tokio::test]
async fn claude_empty_composer_submits_and_promotes_an_existing_stash() {
    for hidden in [None, Some(HIDDEN)] {
        let transfer = transfer(hidden, "", false).await;
        transfer.result.unwrap();
        assert_eq!(transfer.submitted, [MESSAGE]);
        assert_eq!(transfer.fake.body(), hidden.unwrap_or(""));
        assert_eq!(transfer.fake.stashed(), None);
        assert_eq!(
            transfer.keys,
            [
                Bytes::from_static(MESSAGE.as_bytes()),
                Bytes::from_static(b"\r")
            ]
        );
    }
}

#[tokio::test]
async fn claude_late_native_stash_replays_buffered_edit() {
    const ORIGINAL: &str = "fake:draft-bytes ACTIVE CLAUDE-1";
    let mut transfer = transfer(None, ORIGINAL, true).await;
    transfer.result.unwrap();
    assert_eq!(transfer.submitted, [MESSAGE]);
    assert_eq!(transfer.fake.body(), "fake:draft-bytes ACTIVE CLAUDE-1-2");
    assert!(transfer.fake.input(b"-END").is_none());
    assert_eq!(
        transfer.fake.input(b"\r").as_deref(),
        Some("fake:draft-bytes ACTIVE CLAUDE-1-2-END")
    );
}
