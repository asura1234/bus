//! Guarded observation of the version-scoped Grok composer contract.
use super::styled_chars;
use crate::terminal::runtime::InputObservation;

pub(super) fn observe(screen: &str, expected: &str) -> InputObservation {
    let Some(body) = input(screen) else {
        return InputObservation::Unavailable;
    };
    if body.is_empty() {
        return InputObservation::Empty;
    }
    // No captured native paste-chip contract attests the hidden contents.
    // Even a matching count can describe an unrelated developer paste.
    if body.contains("[Pasted") || body.contains("[Image #") {
        return InputObservation::Unavailable;
    }
    let expected = expected.replace("\r\n", "\n").replace('\r', "\n");
    if body == expected {
        InputObservation::Pending
    } else {
        // Other authorizes the shared runtime to clear the draft. Preserve it.
        // Ambiguous soft wraps cannot establish ownership either.
        InputObservation::Unavailable
    }
}

pub(super) fn input(screen: &str) -> Option<String> {
    let styled: Vec<_> = screen.lines().map(styled_chars).collect();
    let lines: Vec<String> = styled
        .iter()
        .map(|line| line.iter().map(|c| c.character).collect())
        .collect();
    let end = lines.iter().rposition(|line| !line.trim().is_empty())?;
    let footer = lines[end].trim().to_lowercase();
    if !footer.contains("shift+tab:mode")
        || !(footer.contains("ctrl+x:shortcuts") || footer.contains("ctrl+.:shortcuts"))
    {
        return None;
    }
    let bottom = (0..end)
        .rev()
        .find(|&index| !lines[index].trim().is_empty())?;
    if !crate::agents::grok_surface::composer_border(&lines[bottom]) {
        return None;
    }
    let top = (0..bottom)
        .rev()
        .find(|&index| crate::agents::grok_surface::composer_border(&lines[index]))?;
    if top + 1 >= bottom {
        return None;
    }
    // Login surfaces must not become ready just because an input box is visible.
    if lines[..top].iter().any(|line| {
        matches!(
            line.trim().to_lowercase().as_str(),
            "log in to grok" | "sign in to grok" | "login required" | "login with grok"
        )
    }) {
        return None;
    }
    let first = content(&lines[top + 1])?.trim_start().strip_prefix('❯')?;
    let mut body = first.strip_prefix(' ').unwrap_or(first).to_owned();
    let indent = if lines[top + 1].trim_start().starts_with('│') {
        "   "
    } else {
        "  "
    };
    for line in &lines[top + 2..bottom] {
        body.push('\n');
        let text = content(line)?;
        // A blank framed row loses its fixed indentation when frame padding
        // is trimmed. Preserve the logical newline; never merge it away.
        body.push_str(
            text.strip_prefix(indent)
                .or_else(|| text.is_empty().then_some(""))?,
        );
    }
    let placeholder = matches!(body.as_str(), "Type a message..." | "Type a message…")
        && styled[top + 1]
            .iter()
            .filter(|c| !c.character.is_whitespace() && !matches!(c.character, '❯' | '│'))
            .all(|c| c.faint);
    if placeholder {
        Some(String::new())
    } else {
        Some(body)
    }
}

fn content(line: &str) -> Option<&str> {
    let line = line.trim_end();
    // Preserve typed indentation after the fixed frame/marker padding.
    match line.trim_start().strip_prefix('│') {
        Some(inner) => Some(inner.strip_suffix('│')?.trim_end()),
        None => Some(line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const EMPTY: &str = include_str!("../../../../../tests/fixtures/grok-composer/empty.txt");
    const DRAFT: &str = include_str!("../../../../../tests/fixtures/grok-composer/draft.txt");
    const PASTED: &str = include_str!("../../../../../tests/fixtures/grok-composer/pasted.txt");

    #[test]
    fn grok_visible_blank_lines_in_bootstrap_remain_exact() {
        let expected = "line one\n\n  line two\n";
        let screen = format!(
            "╭────────╮\n│ ❯ {} │\n╰────────╯\nShift+Tab:mode │ Ctrl+x:shortcuts\n",
            expected.replace('\n', " │\n│   ")
        );
        assert_eq!(input(&screen).as_deref(), Some(expected));
        assert_eq!(observe(&screen, expected), InputObservation::Pending);
        assert_eq!(
            observe(&screen, "line one line two"),
            InputObservation::Unavailable
        );
    }

    #[test]
    fn grok_exact_body_never_merges_commands_or_trims_user_indent() {
        let screen = DRAFT
            .replace("1. inspect the scratch project", "echo one")
            .replace("2. report results", "echo two");
        assert_eq!(
            observe(&screen, "echo one echo two"),
            InputObservation::Unavailable
        );
        let screen = EMPTY.replace("\x1b[2mType a message...\x1b[0m", "code");
        assert_eq!(observe(&screen, "  code"), InputObservation::Unavailable);
    }

    #[test]
    fn grok_literal_placeholder_is_a_developer_draft() {
        let screen = EMPTY.replace("\x1b[2m", "").replace("\x1b[0m", "");
        assert_eq!(input(&screen).as_deref(), Some("Type a message..."));
        assert_eq!(
            observe(&screen, "another prompt"),
            InputObservation::Unavailable
        );
    }

    #[tokio::test]
    async fn grok_real_submission_preserves_unrelated_drafts_and_uncertain_chips() {
        use bytes::Bytes;
        use std::time::{Duration, Instant};
        for screen in [
            DRAFT.to_owned(),
            PASTED.to_owned(),
            EMPTY.replace("\x1b[2m", "").replace("\x1b[0m", ""),
        ] {
            let (runtime, mut writes) =
                crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 16);
            runtime.test_process_pty_bytes(
                format!("\x1b[2J\x1b[H{}", screen.replace('\n', "\r\n")).as_bytes(),
            );
            let before = runtime.visible_ansi();
            let completion = runtime
                .queue_observed_input_submission(
                    Bytes::from_static(b"Bus task"),
                    Bytes::from_static(b"\r"),
                    vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
                    Duration::ZERO,
                    Some(Instant::now() + Duration::from_secs(1)),
                    super::super::composer::observer(
                        crate::agents::AgentKind::Grok,
                        "Bus task".into(),
                    ),
                )
                .unwrap();
            let error = completion
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap_err();
            assert!(
                crate::terminal::runtime::PromptNotShown::is(&error),
                "{error}"
            );
            assert!(
                writes.try_recv().is_err(),
                "unrelated draft received clear, paste or Enter bytes"
            );
            assert_eq!(runtime.visible_ansi(), before);
        }
    }

    #[tokio::test]
    async fn grok_uncertain_paste_after_write_is_retained_without_enter_or_retry() {
        use bytes::Bytes;
        use std::time::{Duration, Instant};
        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        let draw = |screen: &str| {
            runtime.test_process_pty_bytes(
                format!("\x1b[2J\x1b[H{}", screen.replace('\n', "\r\n")).as_bytes(),
            )
        };
        draw(EMPTY);
        assert_eq!(
            observe(&runtime.visible_ansi(), "Bus task"),
            InputObservation::Empty
        );
        let completion = runtime
            .queue_observed_input_submission(
                Bytes::from_static(b"Bus task"),
                Bytes::from_static(b"\r"),
                vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
                Duration::ZERO,
                Some(Instant::now() + Duration::from_millis(400)),
                super::super::composer::observer(crate::agents::AgentKind::Grok, "Bus task".into()),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let pasted = loop {
            if let Ok(bytes) = writes.try_recv() {
                break bytes;
            }
            assert!(Instant::now() < deadline, "expected one initial Bus paste");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(pasted, Bytes::from_static(b"Bus task"));
        draw(PASTED);
        let error = completion
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(!crate::terminal::runtime::PromptNotShown::is(&error));
        assert!(
            writes.try_recv().is_err(),
            "uncertain paste received clear, Enter or retry bytes"
        );
        assert_eq!(
            input(&runtime.visible_ansi()).as_deref(),
            Some("[Pasted text: 40000 chars]")
        );
    }

    #[tokio::test]
    async fn grok_emulated_composer_retains_exact_input_body() {
        let (runtime, _) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        runtime.test_process_pty_bytes(
            format!("\x1b[2J\x1b[H{}", DRAFT.replace('\n', "\r\n")).as_bytes(),
        );
        assert_eq!(
            observe(
                &runtime.visible_ansi(),
                "1. inspect the scratch project\n2. report results"
            ),
            InputObservation::Pending
        );
    }

    #[test]
    fn grok_composer_empty_and_numbered_draft_observation() {
        assert_eq!(input(EMPTY), Some(String::new()));
        assert_eq!(observe(EMPTY, "synthetic prompt"), InputObservation::Empty);
        assert_eq!(
            input(DRAFT).as_deref(),
            Some("1. inspect the scratch project\n2. report results")
        );
        assert_eq!(
            observe(DRAFT, "1. inspect the scratch project\n2. report results"),
            InputObservation::Pending
        );
        assert_eq!(observe(DRAFT, "unrelated"), InputObservation::Unavailable);
    }

    #[test]
    fn grok_unrelated_drafts_and_internal_spaces_are_preserved() {
        let draft = DRAFT.replace(
            "1. inspect the scratch project",
            "do not overwrite this draft",
        );
        assert_eq!(observe(&draft, "Bus task"), InputObservation::Unavailable);
        let spaced = EMPTY.replace("Type a message...", "echo 'a  b'");
        assert_eq!(
            observe(&spaced, "echo 'a b'"),
            InputObservation::Unavailable
        );
    }

    #[test]
    fn grok_long_paste_chip_never_claims_owned_pending() {
        // A character count alone cannot attest the hidden paste's contents.
        assert_eq!(input(PASTED).as_deref(), Some("[Pasted text: 40000 chars]"));
        assert_eq!(
            observe(PASTED, &"x".repeat(40000)),
            InputObservation::Unavailable
        );
        assert_eq!(
            observe(PASTED, "short prompt"),
            InputObservation::Unavailable
        );
    }

    #[test]
    fn grok_blocked_login_and_modal_composers_are_unavailable() {
        for blocker in [
            include_str!("../../../../../tests/fixtures/grok-dialog/permission.txt"),
            include_str!("../../../../../tests/fixtures/grok-dialog/trust.txt"),
            include_str!("../../../../../tests/fixtures/grok-dialog/question.txt"),
            include_str!("../../../../../tests/fixtures/grok-dialog/free-text.txt"),
            include_str!("../../../../../tests/fixtures/grok-dialog/parked-focus.txt"),
        ] {
            for screen in [format!("{EMPTY}\n{blocker}"), format!("{blocker}\n{EMPTY}")] {
                // A stale card followed by the current composer is handled separately below.
                if screen.starts_with(EMPTY) {
                    assert_eq!(input(&screen), None);
                    assert_eq!(observe(&screen, "task"), InputObservation::Unavailable);
                }
            }
        }
        for title in ["Log in to Grok", "Hooks", "Session info"] {
            assert_eq!(
                observe(
                    &format!("{EMPTY}\n┌── {title} ──┐\n│ modal │\n└────────────┘"),
                    "task"
                ),
                InputObservation::Unavailable
            );
        }
    }

    #[test]
    fn grok_latest_framed_composer_ignores_historical_options() {
        let screen = format!(
            "{}\n{DRAFT}",
            include_str!("../../../../../tests/fixtures/grok-dialog/permission.txt")
        );
        assert_eq!(
            observe(&screen, "1. inspect the scratch project\n2. report results"),
            InputObservation::Pending
        );
    }

    #[test]
    fn grok_unframed_or_incomplete_composer_is_unavailable() {
        for screen in [
            "❯ task",
            "❯ task\nCtrl+x:shortcuts",
            "╭───╮\n│ ❯ task │\nCtrl+x:shortcuts",
        ] {
            assert_eq!(input(screen), None);
            assert_eq!(observe(screen, "task"), InputObservation::Unavailable);
        }
    }

    #[test]
    fn grok_ansi_composer_matches_exact_multiline_body() {
        let wrapped = DRAFT
            .replace(
                "1. inspect the scratch project",
                "\x1b[32mhello wrapped\x1b[0m",
            )
            .replace("2. report results", "world");
        assert_eq!(
            observe(&wrapped, "hello wrapped\nworld"),
            InputObservation::Pending
        );
        assert_eq!(
            observe(&wrapped, "hellowrappedworld"),
            InputObservation::Unavailable
        );
    }
}
