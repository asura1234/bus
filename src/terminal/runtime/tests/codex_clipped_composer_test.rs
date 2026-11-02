use super::*;
use crate::agents::AgentKind;
use crate::terminal::runtime::SubmissionInspector;
use crate::terminal::runtime::NATIVE_TEST_TIMEOUT;

const TEXT: &str = include_str!("../../../../tests/fixtures/codex-composer/review-reply.txt");
const BEFORE: &str = include_str!("../../../../tests/fixtures/codex-composer/before.txt");
const CLIPPED: &str = include_str!("../../../../tests/fixtures/codex-composer/clipped.txt");

#[path = "codex_clipped_existing_draft_test.rs"]
mod existing_draft;

fn inspector(directory: std::path::PathBuf) -> Box<dyn SubmissionInspector> {
    Box::new(crate::server::api::preserving_observer(
        AgentKind::Codex,
        TEXT.into(),
        directory,
        vec![
            Bytes::from_static(b"\x13"),
            Bytes::from_static(b"\x07"),
            Bytes::from_static(b"\x7f"),
        ],
    ))
}

fn render(runtime: &TerminalRuntime, frame: &str) {
    runtime.test_process_pty_bytes(
        format!("\x1b[2J\x1b[H{}\x1b[41;57H", frame.replace('\n', "\r\n")).as_bytes(),
    );
}

async fn next(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    tokio::time::timeout(Duration::from_secs(5), writes.recv())
        .await
        .expect("provider did not receive the guarded input")
        .unwrap()
}

#[tokio::test]
async fn codex_clipped_review_reply_verifies_once_then_submits_and_releases_next_delivery() {
    let directory = crate::utils::test_temp::unique_temp_path("codex-clipped-review");
    std::fs::create_dir_all(&directory).unwrap();
    let native_file = directory.join("native.txt");
    let exchange = directory.join("exchange");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(95, 44, 16);
    runtime.test_submission_clock().unwrap().pause();
    render(&runtime, BEFORE);
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
    assert_eq!(next(&mut writes).await, paste, "exactly one paste");
    render(&runtime, CLIPPED);
    assert_eq!(
        next(&mut writes).await.as_ref(),
        b"\x07",
        "read the full native draft"
    );
    std::fs::write(&native_file, TEXT).unwrap();
    crate::platform::draft_editor::test_run_at(&exchange, &native_file).unwrap();
    let returned = std::fs::read_to_string(&native_file).unwrap();
    let marker = returned.strip_prefix(TEXT).unwrap();
    assert!(marker.starts_with("__BUS_DRAFT_END_"));
    runtime.test_process_pty_bytes(format!("\x1b[42;3H{marker}").as_bytes());
    assert_eq!(
        next(&mut writes).await.as_ref(),
        vec![0x7f; marker.len()],
        "remove only the verified editor acknowledgment marker"
    );
    render(&runtime, CLIPPED);
    assert_eq!(next(&mut writes).await.as_ref(), b"\r");
    render(&runtime, BEFORE);
    completion
        .recv_timeout(NATIVE_TEST_TIMEOUT)
        .unwrap()
        .unwrap();
    assert!(
        writes.try_recv().is_err(),
        "one readback and one Enter only"
    );
    assert!(!std::fs::read_dir(&exchange).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("draft-")
    }));

    let follow = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(b"next request"),
            Bytes::from_static(b"\r"),
            vec![],
            Duration::ZERO,
            None,
            Box::new(|frame: &str| {
                if frame.contains("next request") {
                    InputObservation::Pending
                } else {
                    InputObservation::Empty
                }
            }),
        )
        .unwrap();
    assert_eq!(next(&mut writes).await.as_ref(), b"next request");
    render(&runtime, "› next request\n\nGPT-6.1\n? for shortcuts");
    assert_eq!(next(&mut writes).await.as_ref(), b"\r");
    render(&runtime, BEFORE);
    follow.recv_timeout(NATIVE_TEST_TIMEOUT).unwrap().unwrap();
    assert!(writes.try_recv().is_err());
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn codex_clipped_foreign_prefix_receives_no_enter_or_destructive_cleanup() {
    let directory = crate::utils::test_temp::unique_temp_path("codex-clipped-foreign");
    std::fs::create_dir_all(&directory).unwrap();
    let native_file = directory.join("native.txt");
    let exchange = directory.join("exchange");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(95, 44, 16);
    runtime.test_submission_clock().unwrap().pause();
    render(&runtime, BEFORE);
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
    assert_eq!(next(&mut writes).await, paste);
    render(&runtime, CLIPPED);
    assert_eq!(next(&mut writes).await.as_ref(), b"\x07");
    // The changed prefix is offscreen: the rendered suffix is identical.
    let foreign = TEXT.replacen("codexrevd:", "developer draft:", 1);
    std::fs::write(&native_file, &foreign).unwrap();
    assert!(crate::platform::draft_editor::test_run_at(&exchange, &native_file).is_err());
    runtime.test_submission_clock().unwrap().rendered();
    let error = completion
        .recv_timeout(NATIVE_TEST_TIMEOUT)
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("Composer changed"), "{error}");
    assert_eq!(std::fs::read_to_string(&native_file).unwrap(), foreign);
    assert!(
        writes.try_recv().is_err(),
        "no Enter, clear, marker deletion or repaste"
    );
    assert!(runtime.terminal.visible_ansi().contains("**基线**"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn codex_verification_busy_after_paste_is_uncertain_and_never_replayed() {
    let directory = crate::utils::test_temp::unique_temp_path("codex-clipped-busy");
    let exchange = directory.join("exchange");
    let occupied =
        crate::platform::draft_editor::Exchange::begin(exchange.clone(), Duration::from_secs(6))
            .unwrap();
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(95, 44, 16);
    runtime.test_submission_clock().unwrap().pause();
    render(&runtime, BEFORE);
    let paste = Bytes::from(format!("\x1b[200~{TEXT}\x1b[201~"));
    let completion = runtime
        .queue_observed_input_with_inspector(
            paste.clone(),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            None,
            inspector(exchange),
        )
        .unwrap();
    assert_eq!(next(&mut writes).await, paste);
    render(&runtime, CLIPPED);
    let error = completion
        .recv_timeout(NATIVE_TEST_TIMEOUT)
        .unwrap()
        .unwrap_err();
    assert!(
        !PromptNotShown::is(&error),
        "an already-pasted draft must never become a definite non-delivery: {error}"
    );
    assert!(error.to_string().contains("after paste"));
    assert!(
        writes.try_recv().is_err(),
        "no Enter, cleanup or second paste"
    );
    drop(occupied);
    std::fs::remove_dir_all(directory).unwrap();
}
