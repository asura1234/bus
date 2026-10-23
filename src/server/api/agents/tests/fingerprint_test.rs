//! Real provider frames, captured only through disposable headless PTYs.
use super::*;

const CODEX_BEFORE: &str =
    include_str!("../../../../../tests/fixtures/dialog-fingerprint/codex-expanded-1.ansi");
const CODEX_AFTER: &str =
    include_str!("../../../../../tests/fixtures/dialog-fingerprint/codex-expanded-2.ansi");
const CODEX_MOVED: &str =
    include_str!("../../../../../tests/fixtures/dialog-fingerprint/codex-selection-moved.ansi");
const CLAUDE_BEFORE: &str =
    include_str!("../../../../../tests/fixtures/dialog-fingerprint/claude-question-1.ansi");
const CLAUDE_AFTER: &str =
    include_str!("../../../../../tests/fixtures/dialog-fingerprint/claude-question-2.ansi");
const CLAUDE_MOVED: &str =
    include_str!("../../../../../tests/fixtures/dialog-fingerprint/claude-selection-moved.ansi");

#[tokio::test]
async fn captured_dialog_fingerprints_ignore_unrelated_redraws() {
    for (before, after) in [(CODEX_BEFORE, CODEX_AFTER), (CLAUDE_BEFORE, CLAUDE_AFTER)] {
        let (mut app, mut writes) =
            app_with_dialog_sized(before.as_bytes(), Some("session"), 134, 45);
        let observed = observe_dialog(&mut app);
        let dialog = observed.dialog.as_ref().unwrap();
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        app.lookup_runtime_sender(0, pane)
            .unwrap()
            .test_process_pty_bytes(format!("\x1b[2J\x1b[H{after}").as_bytes());
        let current = observe_dialog(&mut app);
        assert_ne!(observed.content_revision, current.content_revision);
        assert_eq!(dialog.id, current.dialog.as_ref().unwrap().id);
        assert_eq!(dialog.digest, current.dialog.as_ref().unwrap().digest);
        let accepted = chosen(&choose(
            &mut app,
            &observed,
            Some("session"),
            &dialog.digest,
            1,
        ));
        assert!(accepted.written);
        assert_eq!(writes.recv().await.unwrap(), Bytes::from_static(b"\r"));
    }
}

#[tokio::test]
async fn captured_dialog_fingerprint_logs_explain_selection_only_staleness() {
    if run_logging_test_alone("captured_dialog_fingerprint_logs_explain_selection_only_staleness") {
        return;
    }
    for (before, moved) in [(CODEX_BEFORE, CODEX_MOVED), (CLAUDE_BEFORE, CLAUDE_MOVED)] {
        let (mut app, mut writes) =
            app_with_dialog_sized(before.as_bytes(), Some("session"), 134, 45);
        let capture = crate::utils::logging::test_capture::Capture::default();
        let mut digest = String::new();
        capture.run_filtered(crate::utils::logging::DEV_FILTER, || {
            tracing::callsite::rebuild_interest_cache();
            let observed = observe_dialog(&mut app);
            let dialog = observed.dialog.as_ref().unwrap();
            assert!(dialog.options[0].selected);
            digest.clone_from(&dialog.digest);
            let pane = app.state.workspaces[0].tabs[0].root_pane;
            app.lookup_runtime_sender(0, pane)
                .unwrap()
                .test_process_pty_bytes(format!("\x1b[2J\x1b[H{moved}").as_bytes());
            let rejected = chosen(&choose(&mut app, &observed, Some("session"), &digest, 1));
            assert!(!rejected.written);
            assert!(writes.try_recv().is_err());
        });
        let log = capture.text();
        assert!(
            log.contains("bus.dialog.fingerprint"),
            "missing comparison: {log}"
        );
        assert!(log.contains("phase=\"observe\""), "{log}");
        assert!(log.contains("phase=\"choose\""), "{log}");
        assert!(log.contains("digest_matches=false"), "{log}");
        assert!(
            log.contains(&format!("expected_digest=\"{digest}\"")),
            "{log}"
        );
        for field in [
            "text_hash=",
            "options_hash=",
            "input_hash=",
            "tabs_hash=",
            "hint_hash=",
            "context_hash=",
            "selected=Some(2)",
        ] {
            assert!(log.contains(field), "missing {field}: {log}");
        }
        let observed_line = log
            .lines()
            .find(|line| line.contains("phase=\"observe\"") && line.contains("text_hash="))
            .unwrap();
        let guarded_line = log
            .lines()
            .find(|line| line.contains("phase=\"choose\""))
            .unwrap();
        for field in [
            "dialog_id=",
            "text_hash=",
            "options_hash=",
            "input_hash=",
            "tabs_hash=",
            "hint_hash=",
            "context_hash=",
        ] {
            assert_eq!(
                field_value(observed_line, field),
                field_value(guarded_line, field),
                "only selection moved: {log}"
            );
        }
        assert_ne!(
            field_value(observed_line, "selected="),
            field_value(guarded_line, "selected="),
            "{log}"
        );
        assert!(
            !log.contains("Which capture mode"),
            "question leaked: {log}"
        );
        assert!(!log.contains("ANSI screen"), "option leaked: {log}");
    }
}

#[tokio::test]
async fn captured_dialog_fingerprint_logs_distinguish_closed_questions() {
    if run_logging_test_alone("captured_dialog_fingerprint_logs_distinguish_closed_questions") {
        return;
    }
    let (mut app, mut writes) =
        app_with_dialog_sized(CODEX_BEFORE.as_bytes(), Some("session"), 134, 45);
    let observed = observe_dialog(&mut app);
    let digest = &observed.dialog.as_ref().unwrap().digest;
    let pane = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, pane)
        .unwrap()
        .test_process_pty_bytes(b"\x1b[2J\x1b[H");
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered(crate::utils::logging::DEV_FILTER, || {
        tracing::callsite::rebuild_interest_cache();
        let refused = chosen(&app.handle_agent_dialog_answer(
            "answer".into(),
            answer_params(&observed, Some("private-answer"), false),
        ));
        assert_eq!(refused.reason.as_deref(), Some("stale_or_changed_dialog"));
    });
    assert!(writes.try_recv().is_err());
    let log = capture.text();
    assert!(
        log.contains("bus.dialog.fingerprint"),
        "missing comparison: {log}"
    );
    assert!(log.contains("phase=\"answer\""), "{log}");
    assert!(log.contains("dialog_present=false"), "{log}");
    assert!(
        log.contains(&format!("expected_digest=\"{digest}\"")),
        "{log}"
    );
    assert!(!log.contains("private-answer"), "answer leaked: {log}");
}

fn field_value<'a>(line: &'a str, field: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(field))
        .unwrap()
}

#[tokio::test]
async fn captured_dialog_fingerprint_logs_redact_malformed_expected_digests() {
    if run_logging_test_alone("captured_dialog_fingerprint_logs_redact_malformed_expected_digests")
    {
        return;
    }
    for screen in [CODEX_BEFORE, ""] {
        let (mut app, mut writes) =
            app_with_dialog_sized(screen.as_bytes(), Some("session"), 134, 45);
        let observed = observe_dialog(&mut app);
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run_filtered(crate::utils::logging::DEV_FILTER, || {
            tracing::callsite::rebuild_interest_cache();
            let refused = chosen(&choose(
                &mut app,
                &observed,
                Some("session"),
                "private-malformed-digest",
                1,
            ));
            assert_eq!(refused.reason.as_deref(), Some("stale_or_changed_dialog"));
        });
        assert!(writes.try_recv().is_err());
        let log = capture.text();
        assert!(log.contains("expected_digest_valid=false"), "{log}");
        assert!(!log.contains("private-malformed-digest"), "{log}");
    }
}

/// Per-test scoped tracing filters can race their global interest cache when
/// libtest runs several captures concurrently. Exercise the real dev filter
/// in a child test process, just as nextest isolates individual tests.
fn run_logging_test_alone(name: &str) -> bool {
    const CHILD: &str = "DIALOG_FINGERPRINT_LOG_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .arg(format!(
            "server::api::agents::tests::fingerprint_tests::{name}"
        ))
        .args(["--exact", "--nocapture"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
