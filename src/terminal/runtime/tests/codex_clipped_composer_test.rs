use super::*;
use crate::terminal::runtime::SubmissionInspector;
use crate::terminal::runtime::NATIVE_TEST_TIMEOUT;

const TEXT: &str = include_str!("../../../../tests/fixtures/codex-composer/review-reply.txt");
const BEFORE: &str = include_str!("../../../../tests/fixtures/codex-composer/before.txt");
const CLIPPED: &str = include_str!("../../../../tests/fixtures/codex-composer/clipped.txt");

#[path = "codex_clipped_existing_draft_test.rs"]
pub(crate) mod existing_draft;

type InspectorFactory = fn(std::path::PathBuf) -> Box<dyn SubmissionInspector>;

fn render(runtime: &TerminalRuntime, frame: &str) {
    runtime.test_process_pty_bytes(
        format!("\x1b[2J\x1b[H{}\x1b[41;57H", frame.replace('\n', "\r\n")).as_bytes(),
    );
}

fn render_verified_marker(runtime: &TerminalRuntime, marker: &str) {
    const CAPTURED_MARKER: &str = "__BUS_DRAFT_END_15173-18e4226e01abcbb8-0__";
    const CAPTURED_WRAPPED: &str = "__BUS_DRAFT_END_15173-\n  18e4226e01abcbb8-0__";
    const CAPTURED_SPLIT: usize = 22;
    let captured =
        include_str!("../../../../tests/fixtures/codex-composer/verified-marker-wrapped.txt");
    assert_eq!(CAPTURED_MARKER.len(), 42);
    assert_eq!(CAPTURED_WRAPPED.replace("\n  ", ""), CAPTURED_MARKER);
    assert!(captured.contains(CAPTURED_WRAPPED));
    assert!(marker.is_ascii());
    assert_eq!(CAPTURED_MARKER.find('-').unwrap() + 1, CAPTURED_SPLIT);
    let split = marker.find('-').unwrap() + 1;
    // Preserve the actual native redraw and its observed split after the PID separator,
    // substituting only this exchange's independently returned token.
    let wrapped = format!("{}\n  {}", &marker[..split], &marker[split..]);
    let frame = captured.replace(CAPTURED_WRAPPED, &wrapped);
    render(runtime, &frame);
    runtime.test_process_pty_bytes(format!("\x1b[41;{}H", marker.len() - split + 3).as_bytes());
}

async fn next(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    tokio::time::timeout(Duration::from_secs(5), writes.recv())
        .await
        .expect("provider did not receive the guarded input")
        .unwrap()
}

pub(crate) async fn verifies_once_then_submits_and_releases_next_delivery(
    inspector: InspectorFactory,
) {
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
    render_verified_marker(&runtime, marker);
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

pub(crate) async fn foreign_prefix_receives_no_enter_or_destructive_cleanup(
    inspector: InspectorFactory,
) {
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

pub(crate) async fn verification_busy_after_paste_is_uncertain_and_never_replayed(
    inspector: InspectorFactory,
) {
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
