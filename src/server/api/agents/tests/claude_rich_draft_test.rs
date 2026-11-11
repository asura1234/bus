//! Native paste receipts must not submit the previous empty Claude frame.
use super::*;
use crate::terminal::runtime::TerminalRuntime;
use std::time::Duration;

const ORIGINAL: &str = "[Image #1][Pasted text #2 +60 lines]";
const PASTED: &str = "[Image #3][Pasted text #4 +34 lines]";

fn paint(runtime: &TerminalRuntime, body: &str, stashed: bool, activity: &str) {
    let stash = if stashed { "› stashed" } else { "" };
    let frame = format!("{activity}\n{stash}\n────────────────\n❯ {body}\n────────────────");
    runtime
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{}", frame.replace('\n', "\r\n")).as_bytes());
}

async fn key(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    tokio::time::timeout(Duration::from_secs(5), writes.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn delayed_paste(painted_before_receipt: bool, unrelated_output: bool) {
    let directory = crate::utils::test_temp::unique_temp_path("claude-delayed-rich-paste");
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(100, 24, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    paint(&runtime, ORIGINAL, false, "original draft");
    // This is the real large-paste shape from the native RED: Claude lifts
    // the inline unquoted PNG while collapsing the rest into one text chip.
    let payload = format!(
        "Reply with only STASH_MARKER. Do not use tools.\n\n{}\nDialog screenshot: /private/tmp/native-rich/fixture.png",
        (0..32)
            .map(|i| format!("Preserved report detail {i}: native delivery must bind the correct turn."))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let inspector = PreservingComposer::new(
        AgentKind::Claude,
        payload.clone(),
        directory.clone(),
        vec![
            Bytes::from_static(b"\x13"),
            Bytes::from_static(b"\x07"),
            Bytes::from_static(b"\x7f"),
        ],
    );
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from(payload.clone()),
            Bytes::from_static(b"\r"),
            vec![
                Bytes::from_static(b"\x01"),
                Bytes::from_static(b"\x0b"),
                Bytes::from_static(b"\x15"),
            ],
            Duration::ZERO,
            None,
            Box::new(inspector),
        )
        .unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"\x13");
    let paste_receipt = clock.hold_next_write_completion();
    paint(&runtime, "", true, "draft stashed");
    assert_eq!(key(&mut writes).await.as_ref(), payload.as_bytes());
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"typed meanwhile"))
        .unwrap();
    if painted_before_receipt {
        paint(&runtime, PASTED, true, "native paste finished");
    } else if unrelated_output {
        // Transcript/OSC output advances native content without materializing
        // any input. A sequence change alone cannot authorize Enter.
        paint(&runtime, "", true, "unrelated transcript update");
        runtime.test_process_pty_bytes(b"\x1b]0;unrelated-title\x07");
    }
    paste_receipt.send(()).unwrap();
    let waited_for_input = clock.wait_for_draft_wait_or_enter();
    if !painted_before_receipt && waited_for_input {
        paint(&runtime, PASTED, true, "native paste finished");
    }
    assert_eq!(key(&mut writes).await.as_ref(), b"\r");
    // Complete native restoration even on base, so the failing ordering
    // assertion leaves no writer thread or clock waiter behind.
    paint(&runtime, ORIGINAL, false, "original draft returned");
    completion
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(key(&mut writes).await.as_ref(), b"typed meanwhile");
    assert!(runtime.visible_ansi().contains(ORIGINAL));
    assert!(writes.try_recv().is_err(), "one Enter, no duplicate paste");
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        painted_before_receipt || waited_for_input,
        "Claude received Enter against the empty stashed frame before its paste materialized"
    );
}

#[tokio::test]
async fn claude_rich_paste_waits_for_materialized_input_after_write_receipt() {
    delayed_paste(false, false).await;
}

#[tokio::test]
async fn claude_rich_paste_unrelated_output_is_not_input_readiness() {
    delayed_paste(false, true).await;
}

#[tokio::test]
async fn claude_rich_paste_render_before_flush_receipt_is_not_missed() {
    delayed_paste(true, false).await;
}
