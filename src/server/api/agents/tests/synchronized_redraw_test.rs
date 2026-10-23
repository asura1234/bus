use super::*;
use std::time::Duration;

fn begin_redraw(app: &App, screen: &str) -> usize {
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    let third = screen.len() / 3;
    let third = (0..=third)
        .rev()
        .find(|i| screen.is_char_boundary(*i))
        .unwrap();
    let runtime = app.lookup_runtime_sender(0, pane).unwrap();
    runtime
        .test_process_pty_bytes(format!("\x1b[?2026h\x1b[2J\x1b[H{}", &screen[..third]).as_bytes());
    assert!(runtime.synchronized_output_active());
    third
}

// Adopted from review2-cl's real Codex-frame probe. A never-finished frame
// cannot safely receive keys; it must be explicitly retryable, never Stale.
#[tokio::test]
async fn guarded_choice_during_an_unfinished_synchronized_redraw_of_the_same_dialog_is_not_stale() {
    let (mut app, mut writes) =
        app_with_dialog_sized(CODEX_BEFORE.as_bytes(), Some("session"), 134, 45);
    let observed = observe_dialog(&mut app);
    let digest = observed.dialog.as_ref().unwrap().digest.clone();
    begin_redraw(&app, CODEX_BEFORE);
    let started = std::time::Instant::now();
    let response = choose(&mut app, &observed, Some("session"), &digest, 1);
    let result: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(result["error"]["code"], "dialog_redrawing", "{response}");
    assert!(writes.try_recv().is_err());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "redraw wait must be bounded"
    );
}

#[tokio::test]
async fn guarded_choice_waits_for_the_same_synchronized_redraw_then_sends_once() {
    for replacement in [false, true] {
        let (mut app, mut writes) =
            app_with_dialog_sized(CODEX_BEFORE.as_bytes(), Some("session"), 134, 45);
        let observed = observe_dialog(&mut app);
        let digest = observed.dialog.as_ref().unwrap().digest.clone();
        let next = if replacement {
            CODEX_MOVED
        } else {
            CODEX_BEFORE
        };
        let third = begin_redraw(&app, next);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let writer = app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .test_pty_writer();
        let observed_wait = app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .test_observe_dialog_redraw(Duration::from_secs(2));
        let painter = std::thread::spawn(move || {
            observed_wait
                .recv_timeout(Duration::from_secs(10))
                .expect("guard must observe the unfinished frame before repaint");
            writer(format!("{}\x1b[?2026l", &next[third..]).as_bytes());
        });
        let response = choose(&mut app, &observed, Some("session"), &digest, 1);
        painter.join().unwrap();
        let result = chosen(&response);
        assert_eq!(result.written, !replacement);
        if replacement {
            assert_eq!(result.reason.as_deref(), Some("stale_or_changed_dialog"));
            assert!(writes.try_recv().is_err());
        } else {
            assert_eq!(writes.try_recv().unwrap(), Bytes::from_static(b"\r"));
            assert!(writes.try_recv().is_err(), "exactly one confirmation");
        }
    }
}

#[tokio::test]
async fn guarded_answer_during_synchronized_redraw_is_retryable_then_rechecks_completed_frame() {
    for replacement in [false, true] {
        let (mut app, mut writes) = app_with_dialog(CODEX_TEXT_DIALOG, Some("session"));
        let observed = observe_dialog(&mut app);
        begin_redraw(&app, std::str::from_utf8(CODEX_TEXT_DIALOG).unwrap());
        let response = app.handle_agent_dialog_answer(
            "answer".into(),
            answer_params(&observed, Some("capture answer"), false),
        );
        let result: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(result["error"]["code"], "dialog_redrawing", "{response}");
        assert!(writes.try_recv().is_err());
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let screen = std::str::from_utf8(CODEX_TEXT_DIALOG).unwrap();
        let next = if replacement {
            screen.replace("What token", "Which token")
        } else {
            screen.to_owned()
        };
        let writer = app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .test_pty_writer();
        let observed_wait = app
            .lookup_runtime_sender(0, pane)
            .unwrap()
            .test_observe_dialog_redraw(Duration::from_secs(2));
        let painter = std::thread::spawn(move || {
            observed_wait
                .recv_timeout(Duration::from_secs(10))
                .expect("guard must observe the unfinished frame before repaint");
            writer(format!("\x1b[2J\x1b[H{next}\x1b[?2026l").as_bytes());
        });
        let response = app.handle_agent_dialog_answer(
            "retry".into(),
            answer_params(&observed, Some("capture answer"), false),
        );
        painter.join().unwrap();
        let result = chosen(&response);
        assert_eq!(result.written, !replacement);
        if replacement {
            assert_eq!(result.reason.as_deref(), Some("stale_or_changed_dialog"));
            assert!(writes.try_recv().is_err());
        } else {
            assert!(writes.recv().await.unwrap().ends_with(b"capture answer"));
            assert_eq!(writes.recv().await.unwrap(), Bytes::from_static(b"\r"));
            assert!(
                writes.try_recv().is_err(),
                "answer must not be submitted twice"
            );
        }
    }
}
