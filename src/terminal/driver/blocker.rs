//! Input-control evidence outside editable widgets, never answer policy.
use super::Grid;
use sha2::{Digest, Sha256};
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlKind {
    Foreground,
    Pending,
}

struct Controls {
    kind: ControlKind,
    end: usize,
    menu: Option<Range<usize>>,
    hint: Range<usize>,
}

/// Opaque content identity. A pending answer shortcut can coexist with live
/// activity; only foreground controls interrupt that activity.
pub(crate) fn blocker(grid: &Grid, working: bool) -> Option<String> {
    blocker_from_rows(grid.rows_text(), working)
}

pub(crate) fn blocker_from_text(text: &str, working: bool) -> Option<String> {
    blocker_from_rows(text.lines().map(str::to_owned).collect(), working)
}

fn blocker_from_rows(rows: Vec<String>, working: bool) -> Option<String> {
    let rows = outside_composer(rows);
    let controls = controls(&rows)?;
    if working && controls.kind == ControlKind::Pending {
        return None;
    }
    let range = controls
        .menu
        .clone()
        .unwrap_or_else(|| controls.end.saturating_sub(12)..controls.end + 1);
    let mut hash = Sha256::new();
    for index in range {
        if controls.menu.is_some() && controls.hint.contains(&index) || !meaningful(&rows[index]) {
            continue;
        }
        // Row breaks, padding and focus markers are presentation, not menu
        // content. Dedicated keyboard hints are omitted only for proven menus.
        for character in super::selection::option_body(plain(&rows[index]))
            .0
            .chars()
            .filter(|c| !c.is_whitespace())
        {
            let mut bytes = [0; 4];
            hash.update(character.encode_utf8(&mut bytes).as_bytes());
        }
    }
    Some(format!("{:x}", hash.finalize()))
}

/// Composer ownership must exclude both foreground forms and pending shortcuts.
/// This uses the same classifier as lifecycle projection, with no second VT read.
pub(crate) fn has_input_controls(screen: &str) -> bool {
    has_blocking_input_controls(screen, false)
}

pub(crate) fn has_blocking_input_controls(screen: &str, working: bool) -> bool {
    controls(&outside_composer(
        screen.lines().map(str::to_owned).collect(),
    ))
    .is_some_and(|controls| !working || controls.kind == ControlKind::Foreground)
}

/// The same prompt/border/footer boundaries used by the composer reader.
/// Editable rows cannot introduce controls or change a control fingerprint.
pub(crate) fn editable_composer_region(rows: &[String]) -> Option<Range<usize>> {
    let border = |row: &str| row.starts_with(['─', '╭', '╰', '┌', '└', '▀']);
    let footer = |row: &str| {
        row.starts_with("GPT-")
            || row.starts_with("gpt-")
            || row.starts_with("? for")
            || row.starts_with("/ commands")
    };
    // A numbered choice can itself start with a prompt marker. Only a marker
    // with a real frame counts: a border on the previous row and a border
    // after it, or a footer that actually closes the composer. A draft row
    // that merely shares a footer prefix is not that closer.
    let chrome_after = |start: usize| -> Option<usize> {
        (start + 1..rows.len()).find(|&index| {
            let row = plain(&rows[index]);
            if border(row) {
                return true;
            }
            if !footer(row) {
                return false;
            }
            let later_border = (index + 1..rows.len()).find(|&later| border(plain(&rows[later])));
            if let Some(border_at) = later_border {
                let only_chrome = (index + 1..border_at).all(|later| {
                    let later = plain(&rows[later]);
                    later.is_empty() || footer(later)
                });
                if only_chrome {
                    return false;
                }
            }
            (index + 1..rows.len()).all(|later| {
                let later = plain(&rows[later]);
                later.is_empty() || border(later) || footer(later)
            })
        })
    };
    let start = (0..rows.len()).rev().find(|&start| {
        if !plain(&rows[start]).starts_with(['❯', '›', '>', '→']) {
            return false;
        }
        let Some(end) = chrome_after(start) else {
            return false;
        };
        let framed = start > 0 && border(plain(&rows[start - 1])) && border(plain(&rows[end]));
        framed || footer(plain(&rows[end]))
    })?;
    let end = chrome_after(start)?;
    Some(start..end.saturating_add(1))
}

fn outside_composer(mut rows: Vec<String>) -> Vec<String> {
    if let Some(region) = editable_composer_region(&rows) {
        // Keep the prompt as a boundary against older transcript controls.
        if let Some((column, marker)) = rows[region.start]
            .char_indices()
            .find(|(_, ch)| matches!(ch, '❯' | '›' | '>' | '→'))
        {
            rows[region.start].truncate(column + marker.len_utf8());
        }
        // Blank interior draft rows, including a footer-prefixed line and a
        // hint above the real closer. Leave the closing border or footer row
        // for the control scan: a [y/n] hint can share that row.
        for row in &mut rows[region.start + 1..region.end.saturating_sub(1)] {
            row.clear();
        }
    }
    rows
}

fn plain(row: &str) -> &str {
    row.trim().trim_matches(['│', '┃', '║']).trim()
}

fn meaningful(row: &str) -> bool {
    let row = plain(row);
    !row.is_empty() && !row.chars().all(|ch| "─━═-╭╮╰╯┌┐└┘ ".contains(ch))
}

fn numbered(row: &str) -> bool {
    super::selection::option_body(row).0 != super::selection::option_label(row).0
}

fn body_column(row: &str) -> usize {
    let body = super::selection::option_body(row).0;
    let offset = body.as_ptr() as usize - row.as_ptr() as usize;
    row[..offset].chars().count()
}

/// Scope the latest focused option by the same generic body/label projection
/// used for selection. Indented descriptions may wrap across physical rows.
fn menu(rows: &[String], end: usize) -> Option<Range<usize>> {
    for focus in (0..=end)
        .rev()
        .filter(|index| super::selection::option_body(&rows[*index]).1)
    {
        let column = body_column(&rows[focus]);
        let is_numbered = numbered(&rows[focus]);
        let belongs = |index: usize| {
            !meaningful(&rows[index])
                || (keyboard_hint(&rows[index]).is_none()
                    && !plain(&rows[index]).contains(" · ")
                    && (body_column(&rows[index]) > column
                        || body_column(&rows[index]) == column
                            && numbered(&rows[index]) == is_numbered))
        };
        let mut start = focus;
        while start > 0 && belongs(start - 1) {
            start -= 1;
        }
        let mut finish = focus + 1;
        while finish <= end && belongs(finish) {
            finish += 1;
        }
        let count = (start..finish)
            .filter(|index| {
                meaningful(&rows[*index])
                    && body_column(&rows[*index]) == column
                    && numbered(&rows[*index]) == is_numbered
            })
            .count();
        if count >= 2 {
            while start < finish && !meaningful(&rows[start]) {
                start += 1;
            }
            return Some(start..finish);
        }
    }
    None
}

/// Include the nearest heading/description paragraph, stopping at a visible
/// blank/separator boundary. No fixed physical-row count defines menu identity.
fn menu_context(rows: &[String], menu: Range<usize>) -> Range<usize> {
    let mut start = menu.start;
    while start > 0 && !meaningful(&rows[start - 1]) {
        start -= 1;
    }
    while start > 0 && meaningful(&rows[start - 1]) && keyboard_hint(&rows[start - 1]).is_none() {
        start -= 1;
    }
    start..menu.end
}

fn tail_clear(
    rows: &[String],
    from: usize,
    end: usize,
    menu: Option<&Range<usize>>,
    hint: &Range<usize>,
) -> bool {
    // Menu scoping may already include the rule directly above a metadata row.
    let preceding_rule = rows[..from]
        .iter()
        .rev()
        .find(|row| !plain(row).is_empty())
        .is_some_and(|row| !meaningful(row));
    let mut below_rule = false;
    (from..=end).all(|index| {
        let row = plain(&rows[index]);
        if hint.contains(&index) || menu.is_some_and(|menu| menu.contains(&index)) {
            return true;
        }
        if !meaningful(row) {
            below_rule |= !row.is_empty();
            return true;
        }
        if row == "⟩" {
            return true;
        }
        if super::selection::option_body(row).1 && !numbered(row) {
            return false;
        }
        if keyboard_hint(row).is_some()
            || numbered(row)
            || row.starts_with("? ")
            || ((!hint.is_empty() || preceding_rule) && row.contains(" · "))
        {
            return true;
        }
        // A framed panel may have noninteractive model/context rows beneath
        // its bottom rule. A fresh prompt or an unrelated key hint ends it.
        below_rule && !starts_with_key(&row.to_lowercase())
    })
}

fn controls(rows: &[String]) -> Option<Controls> {
    let end = rows.iter().rposition(|row| meaningful(row))?;
    let menu = menu(rows, end).map(|menu| menu_context(rows, menu));
    let mut pending = None;
    for start in (0..=end).rev() {
        for finish in start..=(start + 2).min(end) {
            // A wrapped keyboard hint is contiguous. Never consume a later
            // editable prompt, option row, blank boundary or activity shortcut.
            if finish > start
                && rows[start + 1..=finish].iter().any(|row| {
                    !meaningful(row)
                        || super::selection::option_body(row).1
                        || numbered(row)
                        || starts_with_key(&plain(row).to_lowercase())
                            && keyboard_hint(row).is_none()
                })
            {
                break;
            }
            let Some(kind) = input_hint(rows, start..finish + 1) else {
                continue;
            };
            let hint = start..finish + 1;
            if kind == ControlKind::Pending {
                let question = rows[..start].iter().rev().take(2).any(|row| {
                    plain(row)
                        .strip_prefix("? ")
                        .is_some_and(|tail| tail.starts_with(|ch: char| ch.is_ascii_digit()))
                });
                if pending.is_none() && (question || tail_clear(rows, finish + 1, end, None, &hint))
                {
                    pending = Some(Controls {
                        kind,
                        end: finish,
                        menu: None,
                        hint,
                    });
                }
                continue;
            }
            if tail_clear(rows, finish + 1, end, menu.as_ref(), &hint) {
                let menu = menu
                    .clone()
                    .filter(|menu| tail_clear(rows, menu.end, end, None, &hint));
                return Some(Controls {
                    kind,
                    end: finish,
                    menu,
                    hint,
                });
            }
        }
    }
    if let Some(menu) = menu.filter(|menu| {
        rows[menu.clone()]
            .iter()
            .filter(|row| numbered(row))
            .count()
            >= 2
            && rows[menu.clone()]
                .iter()
                .any(|row| numbered(row) && super::selection::option_body(row).1)
            && tail_clear(rows, menu.end, end, None, &(0..0))
    }) {
        return Some(Controls {
            kind: ControlKind::Foreground,
            end: menu.end - 1,
            menu: Some(menu),
            hint: 0..0,
        });
    }
    // Two aligned, separately labelled y/n shortcuts are an input form even
    // without a highlighted row or an Enter hint.
    for index in (1..=end).rev() {
        if let (Some((left, 'y')), Some((right, 'n'))) =
            (binary_key(&rows[index - 1]), binary_key(&rows[index]))
        {
            if left == right
                && rows[index + 1..=end]
                    .iter()
                    .all(|row| !super::selection::option_body(row).1)
            {
                return Some(Controls {
                    kind: ControlKind::Foreground,
                    end: index,
                    menu: None,
                    hint: 0..0,
                });
            }
        }
    }
    pending
}

fn binary_key(row: &str) -> Option<(usize, char)> {
    let row = row.trim_end();
    let key = row.chars().next_back()?;
    if !matches!(key, 'y' | 'n') {
        return None;
    }
    let prefix = &row[..row.len() - 1];
    (prefix.len() - prefix.trim_end().len() >= 2 && !prefix.trim().is_empty())
        .then_some((prefix.chars().count(), key))
}

fn starts_with_key(row: &str) -> bool {
    [
        "enter",
        "esc",
        "press ",
        "use arrow",
        "↑",
        "↓",
        "←",
        "→",
        "↵",
        "⏎",
        "space",
        "tab",
        "shift+",
        "ctrl+",
        "⇧",
        "⌃",
    ]
    .iter()
    .any(|key| row.starts_with(key))
}

fn key_list(keys: &str) -> bool {
    let normalized = keys
        .trim()
        .replace("⇧", "shift+")
        .replace("⌃", "ctrl+")
        .replace(['↵', '⏎'], "enter")
        .replace('↑', "up")
        .replace('↓', "down")
        .replace('←', "left")
        .replace('→', "right");
    normalized.split('/').all(|key| {
        super::input::parse_chord(key.trim()).is_some()
            || key.split_once('-').is_some_and(|(first, last)| {
                !first.is_empty()
                    && !last.is_empty()
                    && first.bytes().all(|ch| ch.is_ascii_digit())
                    && last.bytes().all(|ch| ch.is_ascii_digit())
            })
    })
}

fn keyboard_hint(row: &str) -> Option<ControlKind> {
    keyboard_hint_with_form(row, false)
}

fn input_hint(rows: &[String], hint: Range<usize>) -> Option<ControlKind> {
    let text = rows[hint.clone()]
        .iter()
        .map(|row| plain(row))
        .collect::<Vec<_>>()
        .join(" ");
    keyboard_hint_with_form(&text, checkbox_form(&rows[..hint.start]))
}

fn checkbox_form(rows: &[String]) -> bool {
    let mut count = 0;
    for row in rows.iter().rev() {
        let (label, selected) = super::selection::option_label(row);
        let checkbox = ["[ ]", "[x]", "[X]", "[✓]", "[✔]"].iter().any(|marker| {
            label.strip_prefix(marker).is_some_and(|tail| {
                tail.starts_with(char::is_whitespace) && !tail.trim().is_empty()
            })
        });
        if checkbox {
            count += 1;
            if count == 2 {
                return true;
            }
        } else if selected || plain(row) == "⟩" {
            // Current composer/focus separates older transcript fields from
            // the keyboard hint being classified.
            return false;
        }
    }
    false
}

fn keyboard_hint_with_form(row: &str, checkbox_form: bool) -> Option<ControlKind> {
    let row = plain(row).to_lowercase();
    if ["[y/n]", "[n/y]", "(y/n)", "(n/y)", "[yes/no]"]
        .iter()
        .any(|hint| row.contains(hint))
    {
        return Some(ControlKind::Foreground);
    }
    if (row.starts_with("shift+") || row.starts_with('⇧')) && row.contains("to answer") {
        return Some(ControlKind::Pending);
    }
    let action = |text: &str| {
        text.split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|word| {
                [
                    "select", "confirm", "submit", "accept", "continue", "toggle", "next",
                    "navigate", "review", "view",
                ]
                .contains(&word)
            })
    };
    for binding in row.split(['│', '|', '·', ';']) {
        if let Some((key, description)) = binding.trim().split_once(':') {
            if key_list(key)
                && (action(description)
                    || checkbox_form
                        && !description.trim().is_empty()
                        && key
                            .split('/')
                            .all(|key| matches!(key.trim(), "tab" | "space")))
            {
                return Some(ControlKind::Foreground);
            }
        }
    }
    if !starts_with_key(&row) {
        return None;
    }
    let enter = row.contains("enter") || row.contains('↵') || row.contains('⏎');
    let alternate = ["esc", "skip", "cancel", "navigate", "↑", "↓"]
        .iter()
        .any(|key| row.contains(key));
    (enter && action(&row) && (alternate || row.starts_with("press "))
        || (row.contains('↑') || row.contains('↓')) && action(&row)
        || row.starts_with("esc") && row.contains("cancel"))
    .then_some(ControlKind::Foreground)
}
