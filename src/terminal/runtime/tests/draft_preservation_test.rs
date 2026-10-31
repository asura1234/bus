use super::*;
use crate::terminal::runtime::draft_input::{DraftInput, DRAFT_QUIET_WINDOW};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const MESSAGE: &[u8] = b"BUS_PAYLOAD";
const RICH_DRAFT: &str = "SAVED_DRAFT [Image #138] [Pasted text #147 +20 lines]";

struct Preserver {
    native: bool,
    directory: PathBuf,
}

impl SubmissionInspector for Preserver {
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
        Some(if self.native {
            DraftPreservation::Native {
                key: Bytes::from_static(b"\x13"),
                directory: self.directory.clone(),
                visible_draft: RICH_DRAFT.into(),
            }
        } else {
            DraftPreservation::Editor {
                key: Bytes::from_static(b"\x07"),
                backspace: Bytes::from_static(b"\x7f"),
                directory: self.directory.clone(),
            }
        })
    }
    fn native_draft_restored(&self, screen: &str) -> bool {
        self.native && screen.contains(RICH_DRAFT)
    }
    fn restoration_marker_visible(&self, screen: &str, marker: &str) -> bool {
        screen.contains(marker)
    }
    fn preservation_budget(&self) -> Duration {
        Duration::from_secs(20)
    }
}

fn render(runtime: &TerminalRuntime, text: &str) {
    runtime
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{}", text.replace('\n', "\r\n")).as_bytes());
}

fn queue(runtime: &TerminalRuntime, directory: PathBuf, native: bool) -> InputCompletion {
    runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(MESSAGE),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01")],
            Duration::ZERO,
            None,
            Box::new(Preserver { native, directory }),
        )
        .unwrap()
}

async fn next(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    tokio::time::timeout(Duration::from_secs(5), writes.recv())
        .await
        .unwrap()
        .unwrap()
}

fn recovery_files(directory: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("draft-")
        })
        .collect()
}

#[tokio::test]
async fn native_stash_restored_at_submit_is_acceptance_without_a_second_enter() {
    let directory = crate::utils::test_temp::unique_temp_path("native-stash-runtime");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_submission_clock().unwrap().pause();
    render(&runtime, RICH_DRAFT);
    let completion = queue(&runtime, directory.clone(), true);
    assert_eq!(next(&mut writes).await.as_ref(), b"\x13");
    render(&runtime, "EMPTY");
    assert_eq!(next(&mut writes).await.as_ref(), MESSAGE);
    render(&runtime, "BUS_PAYLOAD");
    assert_eq!(next(&mut writes).await.as_ref(), b"\r");
    // Claude restores the full rich draft synchronously, without ever drawing
    // an empty composer or a Working indicator between Enter and restoration.
    render(&runtime, RICH_DRAFT);
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(
        writes.try_recv().is_err(),
        "restored draft must never be submitted"
    );
    assert!(runtime.terminal.visible_ansi().contains(RICH_DRAFT));
    assert!(recovery_files(&directory).is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn recent_developer_keys_defer_all_draft_mutations_until_three_seconds_of_quiet() {
    let directory = crate::utils::test_temp::unique_temp_path("quiet-draft-runtime");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    render(&runtime, RICH_DRAFT);
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"x"))
        .unwrap();
    assert_eq!(next(&mut writes).await.as_ref(), b"x");
    for advance in [
        Duration::ZERO,
        DRAFT_QUIET_WINDOW - Duration::from_millis(1),
    ] {
        clock.advance(advance);
        let error = queue(&runtime, directory.clone(), true)
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap_err();
        assert!(PromptNotShown::composer_busy(&error), "{error}");
        assert!(
            writes.try_recv().is_err(),
            "typing must prohibit stash, clear, paste and Enter"
        );
        assert!(
            !directory.exists(),
            "a deferred attempt must not start draft exchange"
        );
    }
    clock.advance(Duration::from_millis(1));
    let completion = queue(&runtime, directory.clone(), true);
    assert_eq!(next(&mut writes).await.as_ref(), b"\x13");
    render(&runtime, "EMPTY");
    assert_eq!(next(&mut writes).await.as_ref(), MESSAGE);
    render(&runtime, "BUS_PAYLOAD");
    assert_eq!(next(&mut writes).await.as_ref(), b"\r");
    render(&runtime, RICH_DRAFT);
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn editor_roundtrip_preserves_exact_codex_and_cursor_drafts_before_replaying_keys() {
    for codex in [true, false] {
        let directory = crate::utils::test_temp::unique_temp_path("editor-draft-runtime");
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("native-editor.md");
        let exchange = directory.join("exchange");
        let original = format!(
            "\t  中文\n{}\n\ntrailing \t \n\n",
            "  line with spaces  \n".repeat(80)
        );
        let mut draft = original.clone();
        let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 32);
        runtime.test_submission_clock().unwrap().pause();
        render(&runtime, "DRAFT visible end");
        let completion = queue(&runtime, exchange.clone(), false);
        let mut editors = 0;
        let mut enters = 0;
        let mut pastes = 0;
        let mut deletes = 0;
        loop {
            let key = next(&mut writes).await;
            match key.as_ref() {
                b"\x07" => {
                    editors += 1;
                    std::fs::write(&file, &draft).unwrap();
                    crate::platform::draft_editor::test_run_at(&exchange, &file).unwrap();
                    let text = std::fs::read_to_string(&file).unwrap();
                    // Reproduce the installed CLIs' different editor readers.
                    draft = if codex {
                        text.trim_end().to_owned()
                    } else {
                        text.strip_suffix('\n').unwrap_or(&text).to_owned()
                    };
                    render(&runtime, if draft.is_empty() { "EMPTY" } else { &draft });
                    if editors == 1 {
                        assert!(draft.is_empty());
                        runtime
                            .try_send_developer_bytes(Bytes::from_static(b"typed meanwhile"))
                            .unwrap();
                    }
                }
                MESSAGE => {
                    pastes += 1;
                    draft = "BUS_PAYLOAD".into();
                    render(&runtime, &draft);
                }
                b"\r" => {
                    enters += 1;
                    assert_eq!(draft, "BUS_PAYLOAD");
                    draft.clear();
                    render(&runtime, "EMPTY");
                }
                b"typed meanwhile" => {
                    assert_eq!(
                        draft, original,
                        "buffered keys reached an unverified composer"
                    );
                    draft.push_str("typed meanwhile");
                    break;
                }
                bytes if !bytes.is_empty() && bytes.iter().all(|byte| *byte == 0x7f) => {
                    deletes += 1;
                    for _ in bytes {
                        assert!(draft.pop().unwrap().is_ascii());
                    }
                    assert_eq!(
                        draft, original,
                        "remove only the sentinel, preserving all whitespace"
                    );
                    render(&runtime, &draft);
                }
                bytes => panic!("unexpected provider input: {bytes:?}"),
            }
        }
        completion
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!((editors, pastes, enters, deletes), (3, 1, 1, 2));
        assert_eq!(draft, format!("{original}typed meanwhile"));
        assert!(writes.try_recv().is_err());
        assert!(recovery_files(&exchange).is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn draft_lease_replays_buffered_events_in_order_before_later_developer_input() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(80, 24, 8);
    let lease = DraftInput::acquire(&runtime.draft_input, &runtime.io).unwrap();
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"abc"))
        .unwrap();
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"\x1b[D"))
        .unwrap();
    assert!(
        writes.try_recv().is_err(),
        "developer keys must stay out of the transfer"
    );
    lease.restored();
    drop(lease);
    assert_eq!(next(&mut writes).await.as_ref(), b"abc\x1b[D");
    assert!(
        !runtime
            .startup_suggestion_open
            .load(std::sync::atomic::Ordering::Acquire),
        "replaying buffered bytes onto the pane closes the launch window"
    );
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"!"))
        .unwrap();
    assert_eq!(next(&mut writes).await.as_ref(), b"!");
}

struct PlaceholderInspector;

impl SubmissionInspector for PlaceholderInspector {
    fn observe(&self, screen: &str) -> InputObservation {
        self.observe_with_launch(screen, None, false)
    }

    fn observe_with_launch(
        &self,
        screen: &str,
        _cursor: Option<(u16, u16)>,
        pristine: bool,
    ) -> InputObservation {
        if screen.contains("from bus") {
            InputObservation::Accepted
        } else if pristine && screen.contains("Try \"fix typecheck errors\"") {
            InputObservation::Empty
        } else {
            InputObservation::Other
        }
    }
}

fn paint_try_quote(runtime: &TerminalRuntime) {
    let rule = "─".repeat(20);
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");
    runtime.test_process_pty_bytes(
        format!("{rule}\r\n❯\u{a0}Try \"fix typecheck errors\"").as_bytes(),
    );
    runtime.test_process_pty_bytes(b"\x1b[2;3H");
}

async fn paste_prompt(
    runtime: &TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
) -> (Vec<Bytes>, std::io::Result<()>) {
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(b"from bus"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01")],
            Duration::ZERO,
            Some(std::time::Instant::now() + Duration::from_secs(4)),
            Box::new(PlaceholderInspector),
        )
        .unwrap();
    let result = completion
        .recv_timeout(Duration::from_secs(6))
        .unwrap_or_else(|error| Err(std::io::Error::other(error)));
    let mut queued = Vec::new();
    while let Ok(bytes) = writes.try_recv() {
        queued.push(bytes);
    }
    (queued, result)
}

struct RestoreBusDataDir(Option<std::ffi::OsString>);

impl Drop for RestoreBusDataDir {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => std::env::set_var("BUS_DATA_DIR", value),
            None => std::env::remove_var("BUS_DATA_DIR"),
        }
    }
}

#[tokio::test]
async fn buffered_developer_byte_discarded_by_the_lease_keeps_the_placeholder_deliverable() {
    let root = crate::utils::test_temp::unique_temp_path("lease-window");
    std::fs::create_dir_all(&root).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();

    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(80, 12, 8);
    paint_try_quote(&runtime);
    let (queued, result) = paste_prompt(&runtime, &mut writes).await;
    assert!(
        queued.iter().any(|bytes| bytes.as_ref() == b"from bus"),
        "control: untouched placeholder was not pasted: {queued:?} {result:?}"
    );

    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(80, 12, 8);
    paint_try_quote(&runtime);
    let lease = DraftInput::acquire(&runtime.draft_input, &runtime.io).unwrap();
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"T"))
        .unwrap();
    assert!(
        writes.try_recv().is_err(),
        "the byte must stay in the preservation buffer"
    );
    {
        // recover_input writes under data_dir(). Hold the shared env lock only
        // around that drop so a parallel test cannot redirect the recovery file
        // onto the live Bus, and so this sleep does not block other tests.
        let _env = crate::utils::test_env::mutex()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let previous = std::env::var_os("BUS_DATA_DIR");
        std::env::set_var("BUS_DATA_DIR", &root);
        let _restore = RestoreBusDataDir(previous);
        drop(lease);
    }
    assert!(
        writes.try_recv().is_err(),
        "a lease that is not replay-safe must not write the buffered byte"
    );
    std::thread::sleep(DRAFT_QUIET_WINDOW + Duration::from_millis(200));
    let open = runtime
        .startup_suggestion_open
        .load(std::sync::atomic::Ordering::Acquire);
    let (queued, result) = paste_prompt(&runtime, &mut writes).await;
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        open && queued.iter().any(|bytes| bytes.as_ref() == b"from bus"),
        "a developer byte that never reached the pane left the placeholder undeliverable: open={open} queued={queued:?} {result:?}"
    );
}

#[tokio::test]
async fn an_opaque_draft_is_untouched_and_the_next_empty_composer_attempt_delivers() {
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    runtime.test_submission_clock().unwrap().pause();
    render(&runtime, "[Image #138] [Pasted Content 2048 chars]");
    let submit = || {
        runtime
            .queue_observed_input_submission(
                Bytes::from_static(MESSAGE),
                Bytes::from_static(b"\r"),
                vec![Bytes::from_static(b"\x01")],
                Duration::ZERO,
                None,
                |screen| {
                    if screen.contains("EMPTY") {
                        InputObservation::Empty
                    } else if screen.contains("BUS_PAYLOAD") {
                        InputObservation::Pending
                    } else {
                        InputObservation::Other
                    }
                },
            )
            .unwrap()
    };
    let error = submit()
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap_err();
    assert!(
        PromptNotShown::composer_busy(&error),
        "opaque input is a wait: {error}"
    );
    assert!(
        writes.try_recv().is_err(),
        "no editing key may touch opaque input"
    );
    assert!(runtime.terminal.visible_ansi().contains("[Image #138]"));
    // The scheduler's deferred retry sees the developer's now-empty composer.
    render(&runtime, "EMPTY");
    let completion = submit();
    assert_eq!(next(&mut writes).await.as_ref(), MESSAGE);
    render(&runtime, "BUS_PAYLOAD");
    assert_eq!(next(&mut writes).await.as_ref(), b"\r");
    render(&runtime, "EMPTY");
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(writes.try_recv().is_err());
}
