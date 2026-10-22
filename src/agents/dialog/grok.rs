//! Grok card hypotheses, exercised by explicitly synthetic fixtures.
//! Native 1.0.5 card captures are still required before provider acceptance.
use super::{highlighted, Dialog, DialogKind, DialogOption, QuestionInput, Style};

struct Row {
    selector: char,
    glyph: String,
    label: String,
    focused: bool,
    checkbox: bool,
    line: usize,
    label_at: usize,
}

/// Recognize ownership independently from successful option parsing. A malformed
/// current card must still fence legacy menus above it.
pub(super) fn current_card(lines: &[&str]) -> bool {
    let Some(end) = lines.iter().rposition(|line| !line.trim().is_empty()) else {
        return false;
    };
    footer_line(lines[end])
        && lines[..end]
            .iter()
            .any(|line| line.trim_start().starts_with("◆ "))
}

pub(super) fn parse(lines: &[&str], styles: &[Vec<Style>]) -> Option<Dialog> {
    let end = lines.iter().rposition(|line| !line.trim().is_empty())?;
    let mut footer = end;
    while footer > 0 && footer_line(lines[footer - 1]) {
        footer -= 1;
    }
    let hint = lines[footer..=end].join(" ");
    let lower = hint.to_lowercase();
    let parked = lower.contains("tab/space: question") || lower.contains("tab/space: permission");
    let editing = lower.contains("enter:submit") && lower.contains("esc:back");
    let question =
        parked && lower.contains("question") || editing || lower.contains("shift+x:dismiss");
    let permission = lower.contains(":select") && lower.contains("ctrl+c:cancel");
    if !footer_line(lines[end]) || !(parked || editing || question || permission) {
        return None;
    }
    let header = lines[..footer]
        .iter()
        .rposition(|line| line.trim_start().starts_with("◆ "))?;
    let first = (header + 1..footer).find(|&index| row(lines[index], index).is_some())?;
    let rows = collect_rows(lines, first, footer)?;
    let multi = rows.iter().any(|row| row.checkbox);
    validate_rows(&rows, permission, question, editing, multi, &lower)?;
    let focused = focused_row(&rows, styles)?;
    if !parked && focused.is_none() {
        return None;
    }
    let text = lines[header..first]
        .iter()
        .map(|line| {
            line.trim()
                .trim_start_matches("◆ ")
                .trim_start_matches('┃')
                .trim()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    // Preserve all scope, question, row/check/focus/input and style observations.
    let fingerprint_context = format!(
        "{}\0{:?}",
        lines[header..=end].join("\n"),
        styles.get(header..=end)
    );
    if editing {
        let selected = rows.get(focused?)?;
        if selected.selector != 'z' || parked {
            return None;
        }
        let value = if selected.label == "Type your answer here" {
            String::new()
        } else {
            selected.label.clone()
        };
        return Some(Dialog {
            kind: DialogKind::Question,
            input: Some(QuestionInput {
                value,
                skip_key: Some("shift+x"),
            }),
            text,
            options: Vec::new(),
            hint: Some(hint),
            fingerprint_context,
            tabs: Vec::new(),
        });
    }
    if multi && !question {
        return None;
    }
    let options = choice_options(&rows, parked, focused, question, multi, &lower);
    Some(Dialog {
        kind: DialogKind::Choice,
        input: None,
        text,
        options,
        hint: Some(hint),
        fingerprint_context,
        tabs: Vec::new(),
    })
}

fn collect_rows(lines: &[&str], first: usize, footer: usize) -> Option<Vec<Row>> {
    let mut rows: Vec<Row> = Vec::new();
    for (index, line) in lines.iter().enumerate().take(footer).skip(first) {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(next) = row(line, index) {
            if rows.iter().any(|row| row.selector == next.selector) {
                return None;
            }
            rows.push(next);
        } else {
            // A wrapped label must stay within the same gutter, at its label column.
            // Malformed options are not continuations, even when deeply indented.
            let inner = line.trim_start().strip_prefix('┃')?;
            let last = rows.last_mut()?;
            let column = line.chars().take_while(|c| c.is_whitespace()).count()
                + 1
                + inner.chars().take_while(|c| c.is_whitespace()).count();
            if column < last.label_at || inner.trim().is_empty() {
                return None;
            }
            last.label.push(' ');
            last.label.push_str(inner.trim());
        }
    }
    if rows.len() < 2 {
        return None;
    }
    Some(rows)
}

fn validate_rows(
    rows: &[Row],
    permission: bool,
    question: bool,
    editing: bool,
    multi: bool,
    lower: &str,
) -> Option<()> {
    if permission {
        let count = lower.split_whitespace().find_map(|word| {
            word.strip_prefix("1/")?
                .strip_suffix(":select")?
                .parse::<usize>()
                .ok()
        })?;
        if count > 9
            || rows.len() != count
            || multi
            || rows
                .iter()
                .enumerate()
                .any(|(index, row)| row.selector as u32 != '1' as u32 + index as u32)
        {
            return None;
        }
    } else if question && !editing && !multi {
        // Arrow distances are unsafe across an omitted native radio row.
        // Reject observable gaps; the z editor is directly addressable.
        let selectors = "123456789abcdef";
        for (index, row) in rows.iter().enumerate() {
            if row.selector == 'z' {
                if index + 1 != rows.len() {
                    return None;
                }
            } else if selectors.chars().nth(index) != Some(row.selector) {
                return None;
            }
        }
    }
    Some(())
}

fn focused_row(rows: &[Row], styles: &[Vec<Style>]) -> Option<Option<usize>> {
    let row_styles: Vec<_> = rows
        .iter()
        .map(|row| {
            styles
                .get(row.line)
                .and_then(|styles| styles.get(row.label_at))
                .copied()
                .unwrap_or_default()
        })
        .collect();
    let styled_focus = highlighted(&row_styles);
    let explicit: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.focused)
        .map(|(index, _)| index)
        .collect();
    let focused = match explicit.as_slice() {
        [index] if styled_focus.is_none_or(|styled| styled == *index) => Some(*index),
        [] => styled_focus.or_else(|| {
            if rows.iter().any(|row| row.checkbox) {
                return None;
            }
            let filled: Vec<_> = rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.glyph == "(●)")
                .map(|(index, _)| index)
                .collect();
            match filled.as_slice() {
                [index] => Some(*index),
                _ => None,
            }
        }),
        _ => return None,
    };
    Some(focused)
}

fn choice_options(
    rows: &[Row],
    parked: bool,
    focused: Option<usize>,
    question: bool,
    multi: bool,
    lower: &str,
) -> Vec<DialogOption> {
    let mut options: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let selected = !parked && focused == Some(index);
            DialogOption {
                number: index as u32 + 1,
                label: format!("{} {}", row.glyph, row.label),
                identity_label: Some(row.label.clone()),
                selected,
                free_text: false,
                // Stable shortcuts keep selection out of Dialog::id(). They also
                // avoid arrow counts across a hidden row before the z editor.
                navigation_key: if !parked && question {
                    shortcut(row.selector)
                } else {
                    None
                },
            }
        })
        .collect();
    if multi && !parked {
        options.push(action(options.len(), "Submit / advance answers", "enter"));
    }
    if !parked && lower.contains("←/→:question") {
        options.push(action(options.len(), "Previous question", "left"));
        options.push(action(options.len(), "Next question", "right"));
    }
    if multi && !parked {
        options.push(action(options.len(), "Toggle / edit focused answer", " "));
    }
    options
}

fn action(index: usize, label: &str, key: &'static str) -> DialogOption {
    DialogOption {
        number: index as u32 + 1,
        label: label.into(),
        selected: false,
        free_text: false,
        navigation_key: Some(key),
        identity_label: None,
    }
}

fn shortcut(selector: char) -> Option<&'static str> {
    match selector {
        '1' => Some("1"),
        '2' => Some("2"),
        '3' => Some("3"),
        '4' => Some("4"),
        '5' => Some("5"),
        '6' => Some("6"),
        '7' => Some("7"),
        '8' => Some("8"),
        '9' => Some("9"),
        'a' => Some("a"),
        'b' => Some("b"),
        'c' => Some("c"),
        'd' => Some("d"),
        'e' => Some("e"),
        'f' => Some("f"),
        'z' => Some("z"),
        _ => None,
    }
}

fn row(full: &str, line: usize) -> Option<Row> {
    let inner = full.trim_start().strip_prefix('┃')?.trim();
    let (focused, inner) = match inner.strip_prefix('❯') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, inner),
    };
    let (selector, body) = inner.split_once(char::is_whitespace)?;
    let mut chars = selector.chars();
    let selector = chars.next()?;
    if chars.next().is_some() || shortcut(selector).is_none() {
        return None;
    }
    let body = body.trim_start();
    let glyph = ["(○)", "(●)", "[ ]", "[✓]", "[✔]", "[x]"]
        .into_iter()
        .find(|glyph| body.starts_with(glyph))?;
    let after = body.strip_prefix(glyph)?;
    if !after.starts_with(char::is_whitespace) {
        return None;
    }
    let label = after.trim();
    if label.is_empty() {
        return None;
    }
    let offset = label.as_ptr() as usize - full.as_ptr() as usize;
    Some(Row {
        selector,
        glyph: glyph.into(),
        label: label.into(),
        focused,
        checkbox: glyph.starts_with('['),
        line,
        label_at: full[..offset].chars().count(),
    })
}

fn footer_line(line: &str) -> bool {
    let lower = line.trim().to_lowercase();
    !lower.starts_with('┃')
        && [
            ":select",
            ":cancel",
            ":submit",
            "esc:back",
            "space:toggle",
            "enter:next",
            "shift+x:dismiss",
            "ctrl+o:yolo",
            "tab/space: question",
            "tab/space: permission",
        ]
        .iter()
        .any(|hint| lower.contains(hint))
}

/// A complete normal-mode frame suppresses historical and editable numbered menus.
/// A card's active/parked footer must never be mistaken for composer readiness.
pub(super) fn contains_composer(lines: &[&str]) -> bool {
    let Some(end) = lines.iter().rposition(|line| !line.trim().is_empty()) else {
        return false;
    };
    let footer = lines[end].trim().to_lowercase();
    if !footer.contains("shift+tab:mode")
        || !(footer.contains("ctrl+x:shortcuts") || footer.contains("ctrl+.:shortcuts"))
    {
        return false;
    }
    let Some(bottom) = (0..end)
        .rev()
        .find(|&index| !lines[index].trim().is_empty())
    else {
        return false;
    };
    if !crate::agents::grok_surface::composer_border(lines[bottom]) {
        return false;
    }
    let Some(top) = (0..bottom)
        .rev()
        .find(|&index| crate::agents::grok_surface::composer_border(lines[index]))
    else {
        return false;
    };
    let Some(first) = lines.get(top + 1).filter(|_| top + 1 < bottom) else {
        return false;
    };
    first
        .trim()
        .trim_start_matches('│')
        .trim_start()
        .starts_with('❯')
}

#[cfg(test)]
mod tests {
    use super::super::{styled_lines, DialogKind};
    use super::*;
    use crate::agents::{
        manifest::{detect_with_osc, DetectionInput},
        AgentKind, AgentState,
    };

    const PERMISSION: &str = include_str!("../../../tests/fixtures/grok-dialog/permission.txt");
    const TRUST: &str = include_str!("../../../tests/fixtures/grok-dialog/trust.txt");
    const QUESTION: &str = include_str!("../../../tests/fixtures/grok-dialog/question.txt");
    const TEXT: &str = include_str!("../../../tests/fixtures/grok-dialog/free-text.txt");
    const PARKED: &str = include_str!("../../../tests/fixtures/grok-dialog/parked-focus.txt");
    const WRAPPED: &str = include_str!("../../../tests/fixtures/grok-dialog/wrapped.txt");
    const DRAFT: &str = include_str!("../../../tests/fixtures/grok-composer/draft.txt");

    #[tokio::test]
    async fn grok_native_composer_fences_historical_numbered_dialogs() {
        let capture =
            include_str!("../../../tests/fixtures/grok-composer/native-1.0.5-offline.ansi");
        let label = capture.find(":shortcuts").unwrap();
        let end = label + capture[label..].find("\x1b[?2026l").unwrap() + "\x1b[?2026l".len();
        let (runtime, _) = crate::terminal::TerminalRuntime::test_with_channel_capacity(120, 40, 8);
        runtime.test_process_pty_bytes(capture[..end].as_bytes());
        let screen = runtime.visible_ansi();
        let (texts, _) = styled_lines(&screen);
        assert!(
            contains_composer(&texts.iter().map(String::as_str).collect::<Vec<_>>()),
            "{screen:?}"
        );
        let history = "Earlier menu\n❯ 1. Approve\n  2. Deny\nEnter to select\n";
        assert!(super::super::parse(&format!("{history}{screen}")).is_none());
    }

    fn card(screen: &str) -> Option<Dialog> {
        let (text, styles) = styled_lines(screen);
        parse(
            &text.iter().map(String::as_str).collect::<Vec<_>>(),
            &styles,
        )
    }

    #[test]
    fn grok_c_permission_and_trust_use_contiguous_rows_and_arrows() {
        for screen in [PERMISSION, TRUST] {
            let dialog = card(screen).expect("synthetic focused card");
            assert_eq!(dialog.kind, DialogKind::Choice);
            assert_eq!(dialog.selected(), Some(1));
            assert_eq!(dialog.keys_for(2), Some(vec!["down", "enter"]));
            assert_eq!(dialog.keys_for(99), None);
            assert!(super::super::parse(screen).is_some());
        }
    }

    #[test]
    fn grok_c_scope_and_focus_invalidate_observed_digest() {
        let before = card(PERMISSION).unwrap();
        let scope = card(&PERMISSION.replace("this command", "all commands")).unwrap();
        let moved = card(&PERMISSION.replace("❯ 1", "  1").replace("  2", "❯ 2")).unwrap();
        assert_ne!(before.digest(), scope.digest());
        assert_eq!(before.id(), moved.id());
        assert_ne!(before.digest(), moved.digest());
        assert_eq!(moved.keys_for(1), Some(vec!["up", "enter"]));
    }

    #[test]
    fn grok_c_checkboxes_preserve_checked_state_separate_from_focus() {
        let before = card(QUESTION).unwrap();
        let after = card(&QUESTION.replace("[ ] Red", "[✓] Red")).unwrap();
        assert_eq!(before.selected(), Some(1));
        assert!(before.options[1].label.contains("[✓] Blue"));
        assert_eq!(before.id(), after.id());
        assert_ne!(before.digest(), after.digest());
        assert_eq!(before.keys_for(1), Some(vec!["1"]));
        assert_eq!(before.keys_for(2), Some(vec!["2"]));
        assert_eq!(before.options[2].number, 3);
        assert_eq!(before.keys_for(3), Some(vec!["a"]));
        assert_eq!(before.keys_for(5), Some(vec!["enter"]));
        assert_eq!(before.keys_for(6), Some(vec!["left"]));
        assert_eq!(before.keys_for(7), Some(vec!["right"]));
        assert_eq!(before.keys_for(8), Some(vec![" "]));
    }

    #[test]
    fn grok_c_question_and_tab_style_changes_invalidate_digest() {
        let before = card(QUESTION).unwrap();
        let next = card(&QUESTION.replace("1/2", "2/2")).unwrap();
        let styled = card(&QUESTION.replace("Question 1/2", "\x1b[7mQuestion 1/2\x1b[0m")).unwrap();
        assert_ne!(before.digest(), next.digest());
        assert_ne!(before.digest(), styled.digest());
    }

    #[test]
    fn grok_c_free_text_has_no_options_and_tracks_edits() {
        let before = card(TEXT).unwrap();
        let after = card(&TEXT.replace("ultraviolet please", "infrared")).unwrap();
        assert_eq!(before.kind, DialogKind::Question);
        assert!(before.options.is_empty());
        assert_eq!(before.input.as_ref().unwrap().value, "ultraviolet please");
        assert_eq!(before.input.as_ref().unwrap().skip_key, Some("shift+x"));
        assert_eq!(before.id(), after.id());
        assert_ne!(before.digest(), after.digest());
    }

    #[test]
    fn grok_c_parked_card_has_no_actionable_selection() {
        let dialog = card(PARKED).unwrap();
        assert_eq!(dialog.selected(), None);
        for row in &dialog.options {
            assert_eq!(dialog.keys_for(row.number), None);
        }
        assert!(!contains_composer(&PARKED.lines().collect::<Vec<_>>()));
    }

    #[test]
    fn grok_c_wrapped_labels_remain_one_option() {
        let dialog = card(WRAPPED).unwrap();
        assert_eq!(dialog.options.len(), 2);
        assert_eq!(
            dialog.options[0].label,
            "(○) Yes, proceed with the synthetic command in the scratch directory"
        );
    }

    #[test]
    fn grok_c_history_and_numbered_drafts_are_not_cards() {
        assert!(card(&format!("{PERMISSION}\n{DRAFT}")).is_none());
        assert!(super::super::parse(&format!("{PERMISSION}\n{DRAFT}")).is_none());
        assert!(contains_composer(&DRAFT.lines().collect::<Vec<_>>()));
        assert!(card(&PERMISSION.replace(
            "1/3:select │ ←/→:scope │ Ctrl+o:yolo │ Ctrl+c:cancel",
            "quoted example"
        ))
        .is_none());
    }

    #[test]
    fn grok_c_ambiguous_missing_focus_and_truncated_cards_fail_closed() {
        for screen in [
            PERMISSION.replace("❯ 1", "  1"),
            PERMISSION.replace("  2", "❯ 2"),
            PERMISSION.replace("┃   2 (○) Always allow this command", "┃   2 (?) unknown"),
            PERMISSION.replace("┃   3 (○) No, reject", "┃   2 (○) duplicate"),
        ] {
            assert!(card(&screen).is_none(), "{screen}");
        }
    }

    #[test]
    fn grok_c_radio_and_style_focus_are_not_checkbox_checks() {
        let radio = PERMISSION.replace("❯ 1 (○)", "  1 (●)");
        assert_eq!(card(&radio).unwrap().selected(), Some(1));
        let styled =
            PERMISSION.replace("❯ 1 (○) Yes, proceed", "  1 (○) \x1b[7mYes, proceed\x1b[0m");
        assert_eq!(card(&styled).unwrap().selected(), Some(1));
        assert!(card(&QUESTION.replace("❯ 1", "  1")).is_none());
    }

    #[test]
    fn grok_c_narrow_footer_and_history_before_current_card() {
        let narrow = PERMISSION.replace(" │ Ctrl+o:yolo", "\nCtrl+o:yolo");
        assert_eq!(card(&narrow).unwrap().options.len(), 3);
        let screen = format!("{QUESTION}\n{PERMISSION}");
        assert_eq!(
            card(&screen).unwrap().text,
            "Allow shell command?\nScope: this command"
        );
    }

    fn detected(screen: &str, title: &str) -> crate::agents::AgentDetection {
        detect_with_osc(
            AgentKind::Grok,
            DetectionInput {
                screen,
                osc_title: title,
                osc_progress: "",
            },
        )
    }

    #[test]
    fn grok_c_screen_rules_keep_parked_and_checkbox_cards_blocked() {
        for screen in [PARKED, QUESTION, PERMISSION, TEXT, TRUST] {
            let result = detected(screen, "grok");
            assert_eq!(result.state, AgentState::Blocked, "{screen}");
            assert!(result.visible_blocker);
        }
    }

    #[test]
    fn grok_c_screen_rules_ignore_historical_options_before_current_composer() {
        let result = detected(&format!("{PERMISSION}\n{DRAFT}"), "grok");
        assert!(!result.visible_blocker);
        assert!(result.visible_idle);
    }

    #[test]
    fn grok_c_checkbox_focus_preserves_identity_and_changes_digest() {
        let original = card(QUESTION).unwrap();
        let moved = card(&QUESTION.replace("❯ 1", "  1").replace("  2", "❯ 2")).unwrap();
        assert_eq!(original.id(), moved.id());
        assert_ne!(original.digest(), moved.digest());
    }

    #[test]
    fn grok_c_radio_questions_with_missing_rows_fail_closed() {
        let radio = QUESTION
            .replace("[ ] Red", "(○) Red")
            .replace("[✓] Blue", "(○) Blue")
            .replace("a [ ] Green", "3 (○) Green");
        assert!(card(&radio).is_some());
        assert!(card(&radio.replace("┃   2 (○) Blue\n", "")).is_none());
    }

    #[test]
    fn grok_c_permission_missing_or_reordered_rows_fail_closed() {
        for screen in [
            PERMISSION.replace("┃   2 (○) Always allow this command\n", ""),
            PERMISSION
                .replace("┃ ❯ 1 (○) Yes, proceed", "┃ ❯ 2 (○) Yes, proceed")
                .replace(
                    "┃   2 (○) Always allow this command",
                    "┃   1 (○) Always allow this command",
                ),
            PERMISSION.replace("1/3:select", "1/4:select"),
        ] {
            assert!(
                card(&screen).is_none(),
                "incomplete or reordered permission card: {screen}"
            );
        }
    }

    #[test]
    fn grok_c_wrapped_final_row_stays_blocked_with_idle_osc() {
        for original in [TRUST, QUESTION] {
            let end = original.lines().last().unwrap();
            let screen = original.replace(end, &format!("┃              long synthetic continuation one\n┃              long synthetic continuation two\n┃              long synthetic continuation three\n{end}"));
            assert!(card(&screen).is_some());
            assert!(detected(&screen, "grok").visible_blocker);
        }
    }

    #[test]
    fn grok_c_current_composer_after_history_is_ready_without_osc() {
        let screen = format!("{PERMISSION}\n{DRAFT}");
        assert!(detected(&screen, "").visible_idle);
    }

    #[test]
    fn grok_c_screen_rules_recognize_ctrl_x_without_login_readiness() {
        let result = detected(
            include_str!("../../../tests/fixtures/grok-composer/empty.txt"),
            "",
        );
        assert!(result.visible_idle);
        let login = detected(
            "Log in to Grok\n❯ Type a message...\nShift+Tab:mode │ Ctrl+x:shortcuts",
            "grok",
        );
        assert!(!login.visible_idle);
        let working = detected(
            "⠧ Running synthetic command [stop]\nEsc:cancel │ Ctrl+x:shortcuts",
            "",
        );
        assert!(working.visible_working);
    }
}
