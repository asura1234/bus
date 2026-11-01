//! Guarded observation of the version-scoped Grok composer contract.
use super::styled_chars;
use crate::terminal::runtime::InputObservation;

pub(super) fn observe(screen: &str, expected: &str) -> InputObservation {
    let Some(body) = input(screen) else {
        // Grok 1.0.70 replaces the composer with its queue pane after it
        // accepts a prompt submitted mid-turn ("▾ Queued N" under Tasks).
        // That is acceptance. A dialog that happens to mention the queue
        // is not, and a still-readable draft stays owned by the caller.
        return if provider_queued(screen) && crate::agents::dialog::parse(screen).is_none() {
            InputObservation::Accepted
        } else {
            InputObservation::Unavailable
        };
    };
    if body.is_empty() {
        return InputObservation::Empty;
    }
    let expected = expected.replace("\r\n", "\n").replace('\r', "\n");
    // Only the captured 1.0.70 chip shape is owned by the input lease.
    // Older or foreign paste chips remain unreadable and unavailable.
    if body.starts_with("[Pasted: ") {
        return if native_paste_summary(&body, &expected) {
            InputObservation::Pending
        } else {
            InputObservation::Unavailable
        };
    }
    if body.contains("[Pasted") || body.contains("[Image #") {
        return InputObservation::Unavailable;
    }
    if body == expected || native_visible_draft(screen, &body, &expected) {
        InputObservation::Pending
    } else {
        // Other authorizes the shared runtime to clear the draft. Preserve it.
        // Ambiguous soft wraps cannot establish ownership either.
        InputObservation::Unavailable
    }
}

/// The 1.0.70 composer soft-wraps Bus's own paste and can scroll its prefix
/// out of the frame. The input lease first proved an empty composer, then
/// buffered developer keys while writing one exact payload. A complete visible
/// draft must preserve every non-whitespace character; a scrolled draft must
/// preserve a substantial exact suffix and show the native scroll rail.
fn native_visible_draft(screen: &str, body: &str, expected: &str) -> bool {
    let footer = screen
        .lines()
        .map(|line| {
            styled_chars(line)
                .iter()
                .map(|c| c.character)
                .collect::<String>()
        })
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default();
    if !footer.trim().starts_with("Enter:send") {
        return false;
    }
    let normalized = |text: &str| {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    };
    let expected = normalized(expected);
    let scrolled = body.chars().any(is_scroll_rail);
    let visible = normalized(
        &body
            .chars()
            .filter(|c| !is_scroll_rail(*c))
            .collect::<String>(),
    );
    if scrolled {
        visible.chars().count() >= 80 && expected.ends_with(&visible)
    } else {
        visible == expected
    }
}

fn is_scroll_rail(character: char) -> bool {
    ('▁'..='█').contains(&character)
}

/// The Tasks-panel header Grok 1.0.70 draws for prompts waiting to run.
/// Same shape as the manifest rule `queued_prompts_working`: an optional
/// disclosure marker, the word Queued, and a count of at least 1, with
/// nothing else on the line. A bare `Queued N` is the same acceptance. A
/// transcript sentence does not match.
fn provider_queued(screen: &str) -> bool {
    screen.lines().any(queued_count_line)
}

fn queued_count_line(line: &str) -> bool {
    let line = line.trim();
    let rest = if let Some(marked) = line.strip_prefix('▾').or_else(|| line.strip_prefix('▸')) {
        if !marked.starts_with(|c: char| c.is_whitespace()) {
            return false;
        }
        marked.trim_start()
    } else {
        line
    };
    let mut parts = rest.split_whitespace();
    parts.next() == Some("Queued")
        && parts.next().is_some_and(positive_count)
        && parts.next().is_none()
}

fn positive_count(count: &str) -> bool {
    let mut chars = count.chars();
    matches!(chars.next(), Some(c) if ('1'..='9').contains(&c)) && chars.all(|c| c.is_ascii_digit())
}

/// Ctrl+C on `Waiting for response… [stop]` unsends Bus's prompt into the
/// composer and drops `[stop]`. That body is still Bus's submission. A
/// developer draft does not start with the Bus envelope.
#[cfg(test)]
pub(super) fn restored_bus_prompt(screen: &str) -> bool {
    crate::agents::grok_surface::restored_prompt_clear_keys(screen).is_some()
}

pub(super) fn input(screen: &str) -> Option<String> {
    let styled: Vec<_> = screen.lines().map(styled_chars).collect();
    let lines: Vec<String> = styled
        .iter()
        .map(|line| line.iter().map(|c| c.character).collect())
        .collect();
    let end = lines.iter().rposition(|line| !line.trim().is_empty())?;
    let footer = lines[end].trim().to_lowercase();
    let legacy_footer = footer.contains("shift+tab:mode")
        && (footer.contains("ctrl+x:shortcuts") || footer.contains("ctrl+.:shortcuts"));
    let native_build_footer = footer.strip_prefix("grok build").is_some_and(|version| {
        let version = version.trim();
        !version.is_empty()
            && version.len() <= 32
            && version.chars().any(|c| c.is_ascii_digit())
            && version.chars().all(|c| c.is_ascii_digit() || c == '.')
    });
    let native_input_footer =
        footer.starts_with("enter:send") && footer.contains("shift+enter/opt+enter:newline");
    // The welcome screen ends at the lower border; other
    // layouts have a separate footer beneath the frame.
    let bottom = if lines[end].trim().starts_with(['╰', '└'])
        && crate::agents::grok_surface::composer_border(&lines[end])
    {
        end
    } else {
        if !legacy_footer && !native_build_footer && !native_input_footer {
            return None;
        }
        (0..end)
            .rev()
            .find(|&index| !lines[index].trim().is_empty())?
    };
    if !crate::agents::grok_surface::composer_border(&lines[bottom]) {
        return None;
    }
    let top = (0..bottom)
        .rev()
        .find(|&index| crate::agents::grok_surface::composer_border(&lines[index]))?;
    if top + 1 >= bottom {
        return None;
    }
    if login_surface(&lines[..top]) {
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
    // Grok 1.0.70 fills the idle composer with faint ghost text and the
    // footer "Tab/→:accept suggestion". That is not a developer draft.
    // Non-faint text, and faint text without that hint, stays typed input.
    let suggestion = footer.contains("tab/")
        && footer.contains("accept suggestion")
        && faint_suggestion(&styled, top, bottom);
    if placeholder || suggestion {
        Some(String::new())
    } else {
        Some(body)
    }
}

fn faint_suggestion(styled: &[Vec<super::StyledChar>], top: usize, bottom: usize) -> bool {
    let mut saw_text = false;
    for row in &styled[top + 1..bottom] {
        for cell in row {
            if cell.character.is_whitespace()
                || matches!(cell.character, '│' | '╭' | '╮' | '╯' | '╰' | '─' | '❯')
            {
                continue;
            }
            if !cell.faint {
                return false;
            }
            saw_text = true;
        }
    }
    saw_text
}

/// Grok 1.0.70 collapses a bracketed paste into a chip. The shared submission
/// runtime rejects any pre-existing matching chip, then holds an exclusive
/// input lease from its empty preflight through Enter. Require the native
/// chip's entire summary to match the text Bus just pasted.
///
/// Captured 1.0.70 screens: a paste of more than 10000 UTF-8 bytes is labelled
/// `floor(byte_len / 1000) KB` (truncating decimal kilobytes, not 1024 and not
/// rounded). At or below 10000 bytes a multiline paste is labelled with
/// `split('\n').count()` lines, and a single line stays inline. Accept only
/// that label. The other chip across the boundary is not this paste.
fn native_paste_summary(body: &str, expected: &str) -> bool {
    let Some(label) = body
        .strip_prefix("[Pasted: ")
        .and_then(|label| label.strip_suffix(']'))
    else {
        return false;
    };
    if let Some(lines) = label.strip_suffix(" lines") {
        return expected.len() <= 10_000
            && lines
                .parse::<usize>()
                .is_ok_and(|lines| lines > 1 && lines == expected.split('\n').count());
    }
    if let Some(kib) = label.strip_suffix(" KB") {
        return expected.len() > 10_000
            && kib
                .parse::<usize>()
                .is_ok_and(|kib| kib > 0 && kib == expected.len() / 1000);
    }
    false
}

/// A login title owns only a bounded control region, starting at the viewport
/// or a complete frame boundary. Blank padding does not extend its ownership
/// into a preceding transcript or make a current login writable.
fn login_surface(before_composer: &[String]) -> bool {
    let mut region = Vec::new();
    for line in before_composer
        .iter()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(13)
    {
        if crate::agents::grok_surface::composer_border(line) {
            break;
        }
        region.push(line.trim());
    }
    region.len() <= 12
        && region.last().is_some_and(|heading| {
            matches!(
                heading.to_lowercase().as_str(),
                "log in to grok" | "sign in to grok" | "login required" | "login with grok"
            )
        })
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
    const NATIVE_1070_EMPTY: &str =
        include_str!("../../../../../tests/fixtures/grok-composer/native-1.0.70-empty.txt");
    const NATIVE_1070_PASTED: &str =
        include_str!("../../../../../tests/fixtures/grok-composer/native-1.0.70-pasted.txt");
    const NATIVE_1070_SCROLLED: &str =
        include_str!("../../../../../tests/fixtures/grok-composer/native-1.0.70-scrolled.txt");
    const NATIVE_1070_WRAPPED: &str =
        include_str!("../../../../../tests/fixtures/grok-composer/native-1.0.70-wrapped.txt");
    const PASTE_9350: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-9350-lines.txt"
    );
    const PASTE_10000: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-10000-lines.txt"
    );
    const PASTE_10001: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-10001-10kb.txt"
    );
    const PASTE_16000: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-16000-16kb.txt"
    );
    const PASTE_20480: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-20480-20kb.txt"
    );
    const PASTE_20999: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-20999-20kb.txt"
    );
    const PASTE_21000: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-21000-21kb.txt"
    );
    const PASTE_21217: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-21217-21kb.txt"
    );
    const PASTE_21504: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-21504-21kb.txt"
    );
    const PASTE_22000: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-22000-22kb.txt"
    );
    const PASTE_UNICODE_20000: &str = include_str!(
        "../../../../../tests/fixtures/grok-composer/native-1.0.70-paste-unicode-20000-20kb.txt"
    );

    const NATIVE_1070_WELCOME: &str =
        include_str!("../../../../../tests/fixtures/grok-composer/native-1.0.70-welcome.txt");

    #[test]
    fn grok_1070_welcome_composer_accepts_empty_but_preserves_drafts_and_dialogs() {
        assert_eq!(
            observe(NATIVE_1070_WELCOME, "Bus task"),
            InputObservation::Empty
        );
        let draft = NATIVE_1070_WELCOME.replace("│ ❯ ", "│ ❯ developer draft");
        assert_eq!(observe(&draft, "Bus task"), InputObservation::Unavailable);
        for dialog in [
            include_str!("../../../../../tests/fixtures/grok-dialog/permission.txt"),
            include_str!("../../../../../tests/fixtures/grok-dialog/trust.txt"),
            include_str!("../../../../../tests/fixtures/grok-dialog/question.txt"),
        ] {
            assert_eq!(
                observe(&format!("{NATIVE_1070_WELCOME}\n{dialog}"), "Bus task"),
                InputObservation::Unavailable
            );
        }
        let incomplete = NATIVE_1070_WELCOME.replace(" · always-approve ─╯", " · always-approve");
        assert_eq!(
            observe(&incomplete, "Bus task"),
            InputObservation::Unavailable
        );
        let unlabelled = NATIVE_1070_WELCOME.lines().map(|line| {
            if line.contains(" Grok 4.7 (high) · always-approve ─╯") {
                "  ╰───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────╯"
            } else { line }
        }).collect::<Vec<_>>().join("\n");
        assert_eq!(observe(&unlabelled, "Bus task"), InputObservation::Empty);
    }

    // Captured from a real Grok 1.0.70 welcome composer when the account's
    // remaining allowance fell below 10 percent. No Enter had been sent.
    const WEEKLY_LIMIT: &str =
        include_str!("../../../../../tests/fixtures/grok-composer/native-1.0.70-weekly-limit.txt");

    #[test]
    fn grok_structural_composer_accepts_changing_border_labels() {
        for label in [
            "Weekly limit left: 9% · Grok 4.7 (high)",
            "Grok 4.7 (high) · always-approve",
            "Model Ω / fast · mode: plan · remaining: 9%",
            "",
        ] {
            let screen = WEEKLY_LIMIT.replace("Weekly limit left: 9% · Grok 4.7 (high)", label);
            assert_eq!(
                observe(&screen, "Bus task"),
                InputObservation::Empty,
                "{label}"
            );
            let draft = screen.replacen("│ ❯ ", "│ ❯ Bus task", 1);
            assert_eq!(
                observe(&draft, "Bus task"),
                InputObservation::Pending,
                "{label}"
            );
            assert_eq!(
                observe(&draft, "another task"),
                InputObservation::Unavailable
            );
            let historical_card = format!(
                "{}\n{screen}\nShift+Tab:mode │ Ctrl+.:shortcuts\n",
                include_str!("../../../../../tests/fixtures/grok-dialog/permission.txt")
            );
            assert!(crate::agents::dialog::parse(&historical_card).is_none());
            assert_eq!(
                observe(&historical_card, "Bus task"),
                InputObservation::Empty
            );
        }
    }

    #[test]
    fn grok_structural_composer_requires_a_prompt_and_complete_box() {
        for screen in [
            WEEKLY_LIMIT.replace('❯', " "),
            WEEKLY_LIMIT.replace('╯', " "),
            WEEKLY_LIMIT.replace('╭', " "),
            WEEKLY_LIMIT.replace("│ ❯ ", "│ menu "),
        ] {
            assert_eq!(observe(&screen, "Bus task"), InputObservation::Unavailable);
        }
        for dialog in [
            include_str!("../../../../../tests/fixtures/grok-dialog/permission.txt"),
            include_str!("../../../../../tests/fixtures/grok-dialog/question.txt"),
            include_str!(
                "../../../../../tests/fixtures/grok-dialog/native-1.0.70-startup-trust.txt"
            ),
        ] {
            assert_eq!(
                observe(&format!("{WEEKLY_LIMIT}\n{dialog}"), "Bus task"),
                InputObservation::Unavailable
            );
        }
    }

    #[test]
    fn ctrl_c_restored_prompt_is_an_owned_line_kill() {
        let screen = include_str!(
            "../../../../../tests/fixtures/grok-composer/native-1.0.70-waiting-unsent.txt"
        );
        assert!(restored_bus_prompt(screen));
        let keys = crate::agents::grok_surface::restored_prompt_clear_keys(screen).expect("keys");
        assert!(keys.len() >= 3 && keys.len().is_multiple_of(3));
        assert!(keys
            .chunks(3)
            .all(|chunk| chunk == ["ctrl+a", "ctrl+k", "ctrl+u"]));
        assert!(!restored_bus_prompt(&format!("{screen}\n[stop]")));
        assert!(!restored_bus_prompt(NATIVE_1070_EMPTY));
        let developer = screen.replace("[Bus message", "developer note");
        assert!(!restored_bus_prompt(&developer));
        assert!(crate::agents::grok_surface::restored_prompt_clear_keys(&developer).is_none());
    }

    #[test]
    fn grok_queued_pane_without_a_composer_is_accepted() {
        let queued = "   ▾ Queued 1\n   from bus\n";
        assert_eq!(input(queued), None);
        assert_eq!(observe(queued, "from bus"), InputObservation::Accepted);
        assert_eq!(
            observe("   ▸ Queued 2\n", "from bus"),
            InputObservation::Accepted
        );
        assert_eq!(
            observe("   ▾ Queued 0\n", "from bus"),
            InputObservation::Unavailable
        );
        assert_eq!(
            observe(
                "Bus closed it because Queued 1 was still waiting\n",
                "from bus"
            ),
            InputObservation::Unavailable
        );
        // The draft is still in the composer. The queue header above it is
        // not acceptance, and must not skip withdrawal.
        let drafted = "\
   ▾ Queued 1
  ╭────────────────────────────────────────────────────────────╮
  │ ❯ from bus                                                 │
  ╰──────── Grok 4.7 (high) · always-approve ─╯
  Enter:send  │  Shift+Enter/Opt+Enter:newline  │  Shift+Tab:mode  │  Ctrl+.:shortcuts
";
        assert_eq!(input(drafted).as_deref(), Some("from bus"));
        assert_eq!(observe(drafted, "from bus"), InputObservation::Pending);
    }

    #[test]
    fn grok_1070_native_ready_composer_accepts_bus_prompt() {
        assert_eq!(input(NATIVE_1070_EMPTY), Some(String::new()));
        assert_eq!(
            observe(NATIVE_1070_EMPTY, "Bus task"),
            InputObservation::Empty
        );
    }

    #[test]
    fn grok_1070_native_paste_chip_matches_only_its_summary() {
        let four_lines = "first\nsecond\nthird\nfourth";
        assert_eq!(
            input(NATIVE_1070_PASTED).as_deref(),
            Some("[Pasted: 4 lines]")
        );
        assert_eq!(
            observe(NATIVE_1070_PASTED, four_lines),
            InputObservation::Pending
        );
        assert_eq!(
            observe(NATIVE_1070_PASTED, "three\nlines\nonly"),
            InputObservation::Unavailable
        );
        let large = NATIVE_1070_PASTED.replace("[Pasted: 4 lines]", "[Pasted: 21 KB]");
        assert_eq!(
            observe(&large, &"x".repeat(21 * 1024 + 200)),
            InputObservation::Pending
        );
        assert_eq!(
            observe(&large, &"x".repeat(20 * 1024)),
            InputObservation::Unavailable
        );
    }

    /// Same bytes the capture driver pasted: 78 `x`s and a newline, then a short tail.
    fn captured_multiline_paste(bytes: usize) -> String {
        let full = bytes / 79;
        let rem = bytes % 79;
        let mut payload = String::with_capacity(bytes);
        for _ in 0..full {
            payload.push_str(&"x".repeat(78));
            payload.push('\n');
        }
        payload.push_str(&"x".repeat(rem));
        assert_eq!(payload.len(), bytes);
        payload
    }

    #[test]
    fn grok_1070_captured_paste_labels_match_the_pasted_bytes() {
        // Screens from Grok 1.0.70. The chip is what that build drew for these
        // exact payloads. A KB label is floor(byte_len / 1000) once the paste
        // is longer than 10000 bytes; shorter multiline pastes stay line counts.
        let cases = [
            (PASTE_9350, 9350_usize, "[Pasted: 119 lines]", false),
            (PASTE_10000, 10000, "[Pasted: 127 lines]", false),
            (PASTE_10001, 10001, "[Pasted: 10 KB]", false),
            (PASTE_16000, 16000, "[Pasted: 16 KB]", false),
            (PASTE_20480, 20480, "[Pasted: 20 KB]", false),
            (PASTE_20999, 20999, "[Pasted: 20 KB]", false),
            (PASTE_21000, 21000, "[Pasted: 21 KB]", false),
            (PASTE_21217, 21217, "[Pasted: 21 KB]", false),
            (PASTE_21504, 21504, "[Pasted: 21 KB]", false),
            (PASTE_22000, 22000, "[Pasted: 22 KB]", false),
            (PASTE_UNICODE_20000, 20000, "[Pasted: 20 KB]", true),
        ];
        for (screen, bytes, chip, unicode) in cases {
            let payload = if unicode {
                "é".repeat(bytes / 2)
            } else {
                captured_multiline_paste(bytes)
            };
            assert_eq!(payload.len(), bytes, "{chip}");
            assert_eq!(input(screen).as_deref(), Some(chip), "{bytes}");
            assert_eq!(
                observe(screen, &payload),
                InputObservation::Pending,
                "{chip} for {bytes} bytes"
            );
            let neighbor = if chip.ends_with(" lines]") {
                captured_multiline_paste(bytes - 79)
            } else if unicode {
                "é".repeat(bytes / 2 - 1)
            } else {
                captured_multiline_paste(bytes / 1000 * 1000 - 1)
            };
            assert_eq!(
                observe(screen, &neighbor),
                InputObservation::Unavailable,
                "{chip} must reject {bytes}"
            );
        }
    }

    #[test]
    fn grok_1070_paste_chip_rejects_the_other_label_across_10000_bytes() {
        let at_limit = captured_multiline_paste(10_000);
        let over_limit = captured_multiline_paste(10_001);
        assert_eq!(at_limit.split('\n').count(), over_limit.split('\n').count());
        let kb_for_limit = observe(PASTE_10001, &at_limit);
        let lines_for_over = observe(PASTE_10000, &over_limit);
        let kb_for_single_line = observe(PASTE_10001, &"x".repeat(10_000));
        assert_eq!(
            (kb_for_limit, lines_for_over, kb_for_single_line),
            (
                InputObservation::Unavailable,
                InputObservation::Unavailable,
                InputObservation::Unavailable,
            ),
            "the other label across 10000 bytes must not authorize Enter"
        );
    }

    #[test]
    fn grok_1070_scrolled_owned_draft_keeps_its_visible_suffix() {
        let expected = "[Bus message 12; answer with bus send --reply-to 12 --as 3 --text 'YOUR ANSWER'; run in the foreground and check ok:true and stage:\"replied\" before claiming delivery; if it fails, check the existing answer with bus message status 12 before resending]\nScratch E2E path 1: assign grok-e2e-a a task to run a shell command in this scratch Git project that writes path1.txt containing PATH1_OK, then require its correlated Bus reply. When its reply reaches you, answer this request with the worker message ID, reply stage, and observed file content. Use your Bus CLI and do not ask for tool approval.";
        assert!(input(NATIVE_1070_SCROLLED).is_some());
        assert_eq!(
            observe(NATIVE_1070_SCROLLED, expected),
            InputObservation::Pending
        );
        assert_eq!(
            observe(NATIVE_1070_SCROLLED, &expected.replace("PATH1_OK", "WRONG")),
            InputObservation::Unavailable
        );
    }

    #[test]
    fn grok_1070_wrapped_owned_draft_preserves_all_nonwhitespace_chars() {
        let expected = "[Bus message 6; answer with bus send --reply-to 6 --as 3 --text 'YOUR ANSWER'; run in the foreground and check ok:true and stage:\"replied\" before claiming delivery; if it fails, check the existing answer with bus message status 6 before resending]\nReply exactly SHORT_OK.";
        assert!(input(NATIVE_1070_WRAPPED).is_some());
        assert_eq!(
            observe(NATIVE_1070_WRAPPED, expected),
            InputObservation::Pending
        );
        assert_eq!(
            observe(
                NATIVE_1070_WRAPPED,
                &expected.replace("SHORT_OK", "SHORT_BAD")
            ),
            InputObservation::Unavailable
        );
    }

    #[tokio::test]
    async fn grok_1070_preexisting_matching_chip_is_never_submitted() {
        use bytes::Bytes;
        use std::time::{Duration, Instant};

        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 16);
        runtime.test_process_pty_bytes(
            format!("\x1b[2J\x1b[H{}", NATIVE_1070_PASTED.replace('\n', "\r\n")).as_bytes(),
        );
        let expected = "first\nsecond\nthird\nfourth";
        let completion = runtime
            .queue_observed_input_with_inspector(
                Bytes::copy_from_slice(expected.as_bytes()),
                Bytes::from_static(b"\r"),
                vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
                Duration::ZERO,
                Some(Instant::now() + Duration::from_secs(1)),
                Box::new(super::super::composer::observer(
                    crate::agents::AgentKind::Grok,
                    expected.into(),
                )),
            )
            .unwrap();
        let error = completion
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap_err();
        assert!(crate::terminal::runtime::PromptNotShown::is(&error));
        assert!(
            writes.try_recv().is_err(),
            "pre-existing chip received Bus input"
        );
    }

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
                .queue_observed_input_with_inspector(
                    Bytes::from_static(b"Bus task"),
                    Bytes::from_static(b"\r"),
                    vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
                    Duration::ZERO,
                    Some(Instant::now() + Duration::from_secs(1)),
                    Box::new(super::super::composer::observer(
                        crate::agents::AgentKind::Grok,
                        "Bus task".into(),
                    )),
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
            .queue_observed_input_with_inspector(
                Bytes::from_static(b"Bus task"),
                Bytes::from_static(b"\r"),
                vec![Bytes::from_static(b"\x01"), Bytes::from_static(b"\x0b")],
                Duration::ZERO,
                Some(
                    Instant::now()
                        + crate::protocol::api::submission::maximum_budget(
                            Duration::ZERO,
                            b"Bus task",
                        ),
                ),
                Box::new(super::super::composer::observer(
                    crate::agents::AgentKind::Grok,
                    "Bus task".into(),
                )),
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
            .recv_timeout(
                crate::protocol::api::submission::maximum_budget(Duration::ZERO, b"Bus task")
                    + Duration::from_secs(1),
            )
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
