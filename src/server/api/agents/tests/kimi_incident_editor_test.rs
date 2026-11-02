//! Recorded Kimi editor incidents. The payloads are frozen evidence; the frame
//! mutations are controlled because the historical before/after frames were not logged.
use super::*;
use crate::platform::draft_editor::Exchange;
use crate::terminal::runtime::{EditorSubmission, PromptNotShown, SubmissionInspector};
use bytes::Bytes;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Mutex,
    },
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
    incident_draw(runtime, after, repaint);
}

#[tokio::test]
async fn kimi_incident_36435_owned_840_byte_draft_is_spent_once_before_36589() {
    assert_eq!(STRANDED_36435.len(), 840);
    assert_eq!(FRESH_36589.len(), 204);
    assert!(STRANDED_36435.contains("[Bus message 36433 from codexrevc]"));
    assert!(FRESH_36589.contains("[Bus message 36588 from bus master]"));
    let scratch = IncidentScratch::new();
    let directory = scratch.0.join("exchange");
    let previous = Exchange::begin(directory.clone(), WAIT).unwrap();
    previous.save_recovery(STRANDED_36435).unwrap();
    drop(previous);
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(140, 65, 16);
    runtime.test_submission_clock().unwrap().pause();
    incident_draw(&runtime, STRANDED_36435, false);
    let completion = incident_queue(
        &runtime,
        FRESH_36589,
        incident_observer(FRESH_36589, &directory),
    );
    let file = scratch.0.join("native-editor.txt");
    // Recover only the old attested bytes. The native editor itself withdraws
    // the draft; PTY deletion is limited to its fresh acknowledgment marker.
    incident_exchange(
        &runtime,
        &mut writes,
        &directory,
        &file,
        STRANDED_36435,
        "",
        true,
    )
    .await;
    let withdrawn: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "withdrawn")
        })
        .collect();
    assert_eq!(withdrawn.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&withdrawn[0]).unwrap(),
        STRANDED_36435
    );
    incident_exchange(
        &runtime,
        &mut writes,
        &directory,
        &file,
        "",
        FRESH_36589,
        true,
    )
    .await;
    incident_exchange(
        &runtime,
        &mut writes,
        &directory,
        &file,
        FRESH_36589,
        FRESH_36589,
        false,
    )
    .await;
    assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
    incident_draw(&runtime, "", true);
    completion.recv_timeout(WAIT).unwrap().unwrap();
    assert!(
        writes.try_recv().is_err(),
        "exactly one new submission; no old prompt replay"
    );
    assert!(!Exchange::has_recoverable_draft(&directory));

    // A withdrawn proof cannot authorize another destructive recovery.
    incident_draw(&runtime, STRANDED_36435, false);
    let result = incident_queue(
        &runtime,
        FRESH_36589,
        incident_observer(FRESH_36589, &directory),
    )
    .recv_timeout(WAIT)
    .unwrap();
    assert!(result.is_err());
    assert!(
        writes.try_recv().is_err(),
        "spent proof must not withdraw the old text again"
    );
}

#[tokio::test]
async fn kimi_incident_36435_edited_or_foreign_draft_has_no_destructive_keys() {
    for owned in [true, false] {
        let scratch = IncidentScratch::new();
        let directory = scratch.0.join("exchange");
        if owned {
            let previous = Exchange::begin(directory.clone(), WAIT).unwrap();
            previous.save_recovery(STRANDED_36435).unwrap();
            drop(previous);
        }
        let edited = format!("{STRANDED_36435} developer edit");
        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(140, 65, 16);
        let clock = runtime.test_submission_clock().unwrap();
        clock.pause();
        incident_draw(&runtime, &edited, false);
        let completion = incident_queue(
            &runtime,
            FRESH_36589,
            incident_observer(FRESH_36589, &directory),
        );
        if owned {
            assert_eq!(next_write(&mut writes).await.as_ref(), b"\x07");
            let file = scratch.0.join("native-editor.txt");
            std::fs::write(&file, &edited).unwrap();
            assert!(crate::platform::draft_editor::test_run_at(&directory, &file).is_err());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
            clock.advance(Duration::from_millis(40));
        }
        assert!(completion.recv_timeout(WAIT).unwrap().is_err());
        assert!(
            writes.try_recv().is_err(),
            "no deletion, paste or submit of edited/foreign text"
        );
        if owned {
            assert!(std::fs::read_dir(&directory)
                .unwrap()
                .flatten()
                .any(|entry| {
                    std::fs::read_to_string(entry.path()).ok().as_deref() == Some(STRANDED_36435)
                }));
        } else {
            assert!(!directory.exists());
        }
    }
}

// Pause after the second marker-free full readback has captured its screen,
// before the final guard re-reads the composer. Delegation keeps all ownership
// decisions in the real Kimi inspector and adds no production synchronization.
struct ReadbackBoundary<I> {
    real: I,
    readbacks: AtomicUsize,
    reached: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl<I: SubmissionInspector> SubmissionInspector for ReadbackBoundary<I> {
    fn observe(&self, screen: &str) -> InputObservation {
        self.real.observe(screen)
    }
    fn preservation_budget(&self) -> Duration {
        self.real.preservation_budget()
    }
    fn editor_submission(&self) -> Option<EditorSubmission> {
        self.real.editor_submission()
    }
    fn restoration_marker_visible(&self, screen: &str, marker: &str) -> bool {
        self.real.restoration_marker_visible(screen, marker)
    }
    fn editor_frame_readable(&self, screen: &str) -> bool {
        let readable = self.real.editor_frame_readable(screen);
        if readable
            && !screen.contains("__BUS_DRAFT_END_")
            && self.readbacks.fetch_add(1, Ordering::SeqCst) == 1
        {
            self.reached.send(()).unwrap();
            self.release.lock().unwrap().recv_timeout(WAIT).unwrap();
        }
        readable
    }
    fn editor_frame_unchanged(&self, previous: &str, current: &str) -> bool {
        self.real.editor_frame_unchanged(previous, current)
    }
    fn editor_submit_key(&self, screen: &str, default: &Bytes) -> Bytes {
        self.real.editor_submit_key(screen, default)
    }
    fn editor_working(&self, screen: &str) -> bool {
        self.real.editor_working(screen)
    }
}

#[tokio::test]
async fn kimi_incident_36297_repaint_after_readback_submits_714_bytes_once() {
    incident_transfer_boundary(false).await;
}

#[tokio::test]
async fn kimi_incident_36297_edit_after_readback_stays_uncertain_without_replay() {
    incident_transfer_boundary(true).await;
}

async fn incident_transfer_boundary(edit: bool) {
    assert_eq!(TRANSFER_36297.len(), 714);
    assert!(TRANSFER_36297.contains("[Bus message 36296 from codexshell]"));
    let scratch = IncidentScratch::new();
    let directory = scratch.0.join("exchange");
    let file = scratch.0.join("native-editor.txt");
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(140, 65, 16);
    let clock = runtime.test_submission_clock().unwrap();
    clock.pause();
    incident_draw(&runtime, "", false);
    let (reached_tx, reached) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let completion = incident_queue(
        &runtime,
        TRANSFER_36297,
        ReadbackBoundary {
            real: incident_observer(TRANSFER_36297, &directory),
            readbacks: AtomicUsize::new(0),
            reached: reached_tx,
            release: Mutex::new(release_rx),
        },
    );
    incident_exchange(
        &runtime,
        &mut writes,
        &directory,
        &file,
        "",
        TRANSFER_36297,
        false,
    )
    .await;
    incident_exchange(
        &runtime,
        &mut writes,
        &directory,
        &file,
        TRANSFER_36297,
        TRANSFER_36297,
        false,
    )
    .await;
    reached.recv_timeout(WAIT).unwrap();
    let current = if edit {
        format!("{TRANSFER_36297} developer edit")
    } else {
        TRANSFER_36297.to_owned()
    };
    incident_draw(&runtime, &current, true);
    release.send(()).unwrap();
    if edit {
        let error = completion.recv_timeout(WAIT).unwrap().unwrap_err();
        assert_eq!(error.to_string(), "Composer changed during editor transfer");
        assert!(
            !PromptNotShown::is(&error),
            "a changed verified draft remains uncertain"
        );
        assert!(Exchange::has_recoverable_draft(&directory));
        assert!(std::fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .any(|entry| {
                std::fs::read_to_string(entry.path()).ok().as_deref() == Some(TRANSFER_36297)
            }));
        clock.advance(Duration::from_secs(180));
        assert!(
            writes.try_recv().is_err(),
            "no automatic second paste or Enter after uncertainty"
        );
    } else {
        assert_eq!(next_write(&mut writes).await.as_ref(), b"\r");
        incident_draw(&runtime, "", true);
        completion.recv_timeout(WAIT).unwrap().unwrap();
        assert!(
            writes.try_recv().is_err(),
            "repaint must submit the exact verified payload once"
        );
        assert!(!Exchange::has_recoverable_draft(&directory));
    }
}
