//! A late native editor swap must never become the draft for copy fallback.
use super::preserving_observer;
use crate::{
    agents::AgentKind,
    platform::draft_editor::test_run_at,
    terminal::{runtime, TerminalRuntime},
};
use bytes::Bytes;
use std::{fs, time::Duration};

const HEADER: &str = "[Bus message 16 from you (room 2)]";
const MARKER: &str = "__BUS_DRAFT_END_";
const WAIT: Duration = runtime::NATIVE_TEST_TIMEOUT;

async fn next(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
    tokio::time::timeout(WAIT, writes.recv())
        .await
        .expect("native input")
        .unwrap()
}

async fn late_editor_swap(kind: AgentKind, midflight_edit: bool) {
    let (original, payload) = if kind == AgentKind::Cursor {
        (
            "fake:draft-bytes ACTIVE CURSOR-1",
            "STASH active delivery CURSOR",
        )
    } else {
        (
            "fake:draft-bytes ACTIVE CODEX-1",
            "STASH active delivery CODEX",
        )
    };
    let incoming = format!("{HEADER}\n{payload}");
    let root = crate::utils::test_temp::unique_temp_path("late-native-swap");
    fs::create_dir_all(&root).unwrap();
    let directory = root.join("exchange");
    let file = root.join("native.txt");
    let (at_fallback, resume_fallback) = runtime::editor_fallback_hook::arm(directory.clone());
    let (runtime, mut writes) = TerminalRuntime::test_with_channel_capacity(120, 30, 16);
    let paint = |draft: &str| {
        let draft = draft.replace('\n', "\r\n   ");
        let view = if kind == AgentKind::Cursor {
            format!(" > {draft}\r\n / commands · @ files")
        } else {
            format!("› {draft}\r\n\r\n GPT-6 · /repo")
        };
        runtime.test_process_pty_bytes(format!("\x1b[2J\x1b[H{view}").as_bytes());
    };
    paint(original);
    let clock = runtime.test_submission_clock().unwrap();
    let release_key = clock.hold_next_write_completion();
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from(format!("\x1b[200~{incoming}\x1b[201~")),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
            Duration::ZERO,
            None,
            Box::new(preserving_observer(
                kind,
                incoming.clone(),
                directory.clone(),
                vec![
                    Bytes::from_static(b"\x13"),
                    Bytes::from_static(b"\x07"),
                    Bytes::from_static(b"\x7f"),
                ],
            )),
        )
        .unwrap();
    assert_eq!(next(&mut writes).await, b"\x07".as_slice());
    if midflight_edit {
        runtime
            .try_send_developer_bytes(Bytes::from_static(b"-2"))
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(490)).await;
    assert!(writes.try_recv().is_err(), "developer edit stays buffered");
    release_key.send(()).unwrap();
    tokio::time::timeout(WAIT, at_fallback)
        .await
        .unwrap()
        .unwrap();

    // The editor finishes inside the fallback decision, after the old
    // unlocked absence check and before the old destructive cleanup.
    fs::write(&file, original).unwrap();
    test_run_at(&directory, &file).unwrap();
    let mut draft = fs::read_to_string(&file).unwrap();
    assert!(draft.starts_with(&incoming) && draft.contains(MARKER));
    paint(&draft);
    resume_fallback.send(()).unwrap();

    let mut completion =
        tokio::task::spawn_blocking(move || completion.recv_timeout(WAIT).unwrap());
    let (mut submitted, mut pastes) = (Vec::new(), Vec::new());
    let mut result = None;
    loop {
        let chunk = match result {
            Some(_) => match writes.try_recv() {
                Ok(chunk) => chunk,
                Err(_) => break,
            },
            None => tokio::select! {
                received = &mut completion => {
                    result = Some(received.unwrap());
                    continue;
                }
                chunk = next(&mut writes) => chunk,
            },
        };
        let text = std::str::from_utf8(&chunk).unwrap();
        match text {
            "\x07" => {
                fs::write(&file, &draft).unwrap();
                test_run_at(&directory, &file).unwrap();
                draft = fs::read_to_string(&file).unwrap();
            }
            "\r" => submitted.push(std::mem::take(&mut draft)),
            "\x01" => {}
            "\x0b" | "\x15" => draft.clear(),
            "-2" => draft.push_str(text),
            text if text.bytes().all(|byte| byte == 0x7f) => {
                assert_eq!(draft[draft.find(MARKER).unwrap()..].len(), text.len());
                draft.truncate(draft.len() - text.len());
            }
            text => {
                let pasted = text
                    .strip_prefix("\x1b[200~")
                    .unwrap()
                    .strip_suffix("\x1b[201~")
                    .unwrap();
                pastes.push(pasted.to_owned());
                draft.push_str(pasted);
            }
        }
        paint(&draft);
    }
    result.unwrap().unwrap();
    fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        submitted,
        [incoming.as_str()],
        "one native Enter with the intended bytes"
    );
    assert!(
        !pastes.iter().any(|paste| paste.contains(MARKER)),
        "copy restore replayed an editor payload: {pastes:?}"
    );
    assert!(
        pastes.is_empty(),
        "the editor swap already installed the payload"
    );
    assert!(!draft.contains(HEADER) && !draft.contains(payload) && !draft.contains(MARKER));
    let suffix = if midflight_edit { "-2" } else { "" };
    assert_eq!(draft, format!("{original}{suffix}"));

    runtime
        .try_send_developer_bytes(Bytes::from_static(b"-END"))
        .unwrap();
    assert_eq!(next(&mut writes).await, b"-END".as_slice());
    draft.push_str("-END");
    runtime
        .try_send_developer_bytes(Bytes::from_static(b"\r"))
        .unwrap();
    assert_eq!(next(&mut writes).await, b"\r".as_slice());
    submitted.push(draft);
    assert_eq!(submitted, [incoming, format!("{original}{suffix}-END")]);
}

#[tokio::test]
async fn editor_fallback_race_cursor_preserves_midflight_edit() {
    late_editor_swap(AgentKind::Cursor, true).await;
}

#[tokio::test]
async fn editor_fallback_race_codex_preserves_midflight_edit() {
    late_editor_swap(AgentKind::Codex, true).await;
}

#[tokio::test]
async fn editor_fallback_race_cursor_without_midflight_edit() {
    late_editor_swap(AgentKind::Cursor, false).await;
}
