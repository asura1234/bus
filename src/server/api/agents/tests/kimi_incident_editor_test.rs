//! Recorded Kimi editor incidents. The payloads are frozen evidence; the frame
//! mutations are controlled because the historical before/after frames were not logged.
use super::*;
use crate::terminal::runtime::SubmissionInspector;
use bytes::Bytes;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const WAIT: Duration = crate::terminal::runtime::NATIVE_TEST_TIMEOUT;

async fn next_write(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    tokio::time::timeout(WAIT, writes.recv())
        .await
        .expect("recorded replay native key did not arrive")
        .expect("recorded replay native key channel closed")
}

const STRANDED_36435: &str = include_str!("fixtures/kimi_reliability/editor-request-36435.txt");
const FRESH_36589: &str = include_str!("fixtures/kimi_reliability/editor-request-36589.txt");
const TRANSFER_36297: &str = include_str!("fixtures/kimi_reliability/editor-request-36297.txt");

struct IncidentScratch(PathBuf);

impl IncidentScratch {
    fn new() -> Self {
        let path = crate::utils::test_temp::unique_temp_path("kimi-recorded-editor");
        std::fs::create_dir_all(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(path)
    }
}

impl Drop for IncidentScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// Wrap physical lines without losing the recorded payload's hard newlines.
fn incident_frame(draft: &str, repaint: bool) -> String {
    let mut rows = Vec::new();
    for line in draft.split('\n') {
        let chars: Vec<_> = line.chars().collect();
        if chars.is_empty() {
            rows.push(String::new());
        } else {
            rows.extend(chars.chunks(100).map(|row| row.iter().collect::<String>()));
        }
    }
    let mut frame = format!(
        "{}\r\n╭{}╮\r\n",
        if repaint {
            "unrelated output repainted"
        } else {
            "native output"
        },
        "─".repeat(104)
    );
    for (index, row) in rows.iter().enumerate() {
        frame.push_str(&format!(
            "│ {}{row:<100}  │\r\n",
            if index == 0 { "> " } else { "  " }
        ));
    }
    frame.push_str(&format!(
        "╰{}╯\r\nK3.1 Pro thinking: {}\r\ncontext: {}",
        "─".repeat(104),
        if repaint { "high" } else { "max" },
        if repaint { "4%" } else { "3%" }
    ));
    frame
}

fn incident_draw(runtime: &crate::terminal::TerminalRuntime, draft: &str, repaint: bool) {
    runtime.test_process_pty_bytes(
        format!("\x1b[2J\x1b[H{}", incident_frame(draft, repaint)).as_bytes(),
    );
}

fn incident_observer(body: &str, directory: &Path) -> impl SubmissionInspector {
    preserving_observer(
        AgentKind::Kimi,
        body.to_owned(),
        directory.to_owned(),
        vec![
            Bytes::from_static(b"\x13"),
            Bytes::from_static(b"\x07"),
            Bytes::from_static(b"\x7f"),
        ],
    )
}

fn incident_queue(
    runtime: &crate::terminal::TerminalRuntime,
    body: &str,
    inspector: impl SubmissionInspector + 'static,
) -> crate::terminal::pty::actor::InputCompletion {
    runtime
        .queue_observed_input_with_inspector(
            Bytes::copy_from_slice(body.as_bytes()),
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
        .unwrap()
}

async fn incident_exchange(
    runtime: &crate::terminal::TerminalRuntime,
    writes: &mut tokio::sync::mpsc::Receiver<Bytes>,
    directory: &Path,
    file: &Path,
    before: &str,
    after: &str,
    repaint: bool,
) {
    assert_eq!(next_write(writes).await.as_ref(), b"\x07");
    std::fs::write(file, before).unwrap();
    crate::platform::draft_editor::test_run_at(directory, file).unwrap();
    let returned = std::fs::read_to_string(file).unwrap();
    let marker = returned
        .strip_prefix(after)
        .expect("native editor must return the complete exact payload");
    assert!(marker.starts_with("__BUS_DRAFT_END_"));
    incident_draw(runtime, &returned, repaint);
    assert_eq!(next_write(writes).await.as_ref(), vec![0x7f; marker.len()]);
    std::fs::write(file, after).unwrap();
    incident_draw(runtime, after, repaint);
}

#[tokio::test]
async fn kimi_recorded_payloads_swap_once_and_restore_the_exact_existing_draft() {
    assert_eq!(STRANDED_36435.len(), 840);
    assert_eq!(FRESH_36589.len(), 204);
    assert_eq!(TRANSFER_36297.len(), 714);
    for (original, fresh) in [
        (STRANDED_36435, FRESH_36589),
        ("\t developer draft 中文\ntrailing \t \n", TRANSFER_36297),
    ] {
        let scratch = IncidentScratch::new();
        let directory = scratch.0.join("exchange");
        let file = scratch.0.join("native-editor.txt");
        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(140, 65, 16);
        runtime.test_submission_clock().unwrap().pause();
        incident_draw(&runtime, original, false);
        let completion = incident_queue(&runtime, fresh, incident_observer(fresh, &directory));
        incident_exchange(
            &runtime,
            &mut writes,
            &directory,
            &file,
            original,
            fresh,
            true,
        )
        .await;
        assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
        incident_draw(&runtime, "", true);
        incident_exchange(&runtime, &mut writes, &directory, &file, "", original, true).await;
        incident_exchange(
            &runtime,
            &mut writes,
            &directory,
            &file,
            original,
            original,
            false,
        )
        .await;
        completion.recv_timeout(WAIT).unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
        assert!(
            writes.try_recv().is_err(),
            "exactly one new submission; no old prompt replay"
        );
        assert!(!crate::platform::draft_editor::Exchange::has_preserved_draft(&directory));
        assert!(
            !std::fs::read_dir(&directory)
                .unwrap()
                .flatten()
                .any(|entry| { entry.file_name().to_string_lossy().starts_with("draft-") }),
            "a swapped developer draft is never Bus withdrawal proof"
        );
    }
}
