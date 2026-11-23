use super::{copy_event, paint, CopyLogWriter, Duration, PreservingComposer};
use crate::{
    agents::AgentKind, devtools::tui::FakeClaudeComposer, terminal::runtime::TerminalRuntime,
};
use bytes::Bytes;
use std::{path::PathBuf, sync::mpsc};

const HIDDEN: &str = "fake:draft-bytes HIDDEN";
const VISIBLE: &str = "fake:draft-bytes VISIBLE";
const MESSAGE: &str = "STASH full native slot";

struct Transfer {
    fake: FakeClaudeComposer,
    result: std::io::Result<()>,
    submitted: Vec<String>,
    keys: Vec<Bytes>,
    directory: PathBuf,
    events: mpsc::Receiver<String>,
}

async fn transfer(hidden: Option<&str>, visible: &str, promote_on_clear: bool) -> Transfer {
    let mut fake = FakeClaudeComposer::new();
    if let Some(hidden) = hidden {
        assert!(fake.input(hidden.as_bytes()).is_none());
        assert!(fake.input(b"\x13").is_none());
        assert_eq!(fake.stashed(), Some(hidden));
    }
    assert!(fake.input(visible.as_bytes()).is_none());
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    paint(&runtime, &fake.shown(), fake.stashed().is_some(), "");
    let directory = crate::utils::test_temp::unique_temp_path("claude-occupied-stash");
    let (messages, events) = mpsc::channel();
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .without_time()
            .with_writer(move || CopyLogWriter {
                messages: messages.clone(),
                line: Vec::new(),
            })
            .finish(),
    );
    let completion = tracing::dispatcher::with_default(&dispatch, || {
        runtime
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
            .unwrap()
    });
    drop(dispatch);
    let mut done = tokio::task::spawn_blocking(move || {
        completion.recv_timeout(Duration::from_secs(12)).unwrap()
    });
    let mut submitted = Vec::new();
    let mut keys = Vec::new();
    let result = loop {
        tokio::select! {
            biased;
            Some(bytes) = writes.recv() => {
                if let Some(text) = fake.input(&bytes) {
                    submitted.push(text);
                }
                if promote_on_clear && keys.is_empty() {
                    assert_eq!(bytes.as_ref(), b"\x15");
                    assert!(fake.input(b"\x13").is_none());
                }
                let transcript = submitted.last().map_or_else(String::new, |text| format!("❯ {text}"));
                paint(&runtime, &fake.shown(), fake.stashed().is_some(), &transcript);
                keys.push(bytes);
            }
            result = &mut done => break result.unwrap(),
        }
    };
    Transfer {
        fake,
        result,
        submitted,
        keys,
        directory,
        events,
    }
}

fn backup(transfer: &Transfer, text: &str) {
    let digest = crate::utils::time::digest(text.as_bytes());
    let path = transfer.directory.join(format!("copied-{digest}.txt"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
}

fn remove_directory(directory: PathBuf) {
    if directory.exists() {
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn claude_occupied_stash_copy_restores_both_native_drafts() {
    let mut transfer = transfer(Some(HIDDEN), VISIBLE, false).await;
    assert_eq!(transfer.submitted, [MESSAGE]);
    assert_eq!(transfer.fake.body(), VISIBLE);
    assert_eq!(transfer.fake.stashed(), Some(HIDDEN));
    transfer.result.as_ref().unwrap();
    assert_eq!(transfer.fake.input(b"\r").as_deref(), Some(VISIBLE));
    // The provider promotes a native stash on manual Enter as well.
    assert_eq!(transfer.fake.body(), HIDDEN);
    assert_eq!(transfer.fake.stashed(), None);
    assert_eq!(transfer.fake.input(b"\r").as_deref(), Some(HIDDEN));
    assert_eq!(transfer.fake.input(b"\r"), None);
    copy_event(&transfer.events, "bus.draft.copy_recovery_retired");
    remove_directory(transfer.directory);
}

#[tokio::test]
async fn claude_occupied_stash_copy_keeps_equal_promoted_bytes_out_of_stash() {
    let transfer = transfer(Some(MESSAGE), VISIBLE, false).await;
    assert_eq!(transfer.submitted, [MESSAGE]);
    assert_eq!(transfer.fake.body(), MESSAGE);
    assert_eq!(transfer.fake.stashed(), None);
    assert!(!transfer.keys.iter().any(|key| key.as_ref() == b"\x13"));
    transfer.result.as_ref().unwrap();
    copy_event(&transfer.events, "bus.draft.recovery_required");
    copy_event(&transfer.events, "bus.draft.recovery_required");
    backup(&transfer, VISIBLE);
    backup(&transfer, MESSAGE);
    remove_directory(transfer.directory);
}

#[tokio::test]
async fn claude_occupied_stash_copy_refusal_preserves_promoted_draft() {
    let transfer = transfer(Some(HIDDEN), VISIBLE, true).await;
    assert!(transfer.result.is_err());
    assert!(transfer.submitted.is_empty());
    assert_eq!(transfer.keys, [Bytes::from_static(b"\x15")]);
    assert_eq!(transfer.fake.body(), HIDDEN);
    assert_eq!(transfer.fake.stashed(), None);
    copy_event(&transfer.events, "bus.draft.recovery_required");
    copy_event(&transfer.events, "bus.draft.recovery_required");
    backup(&transfer, VISIBLE);
    backup(&transfer, HIDDEN);
    remove_directory(transfer.directory);
}

#[tokio::test]
async fn claude_occupied_stash_copy_controls_follow_native_behavior() {
    let no_marker = transfer(None, "", false).await;
    no_marker.result.unwrap();
    assert_eq!(no_marker.submitted, [MESSAGE]);
    assert_eq!(no_marker.fake.body(), "");
    assert_eq!(no_marker.fake.stashed(), None);
    remove_directory(no_marker.directory);

    let empty_composer = transfer(Some(HIDDEN), "", false).await;
    empty_composer.result.unwrap();
    assert_eq!(empty_composer.submitted, [MESSAGE]);
    assert_eq!(empty_composer.fake.body(), HIDDEN);
    assert_eq!(empty_composer.fake.stashed(), None);
    remove_directory(empty_composer.directory);
}
