//! Decide whether Kimi's "Running a command" line is live work.
//!
//! Kimi 2.4.1 draws the in-progress Bash header with `boldFg`, and its wire
//! records `agent.turn.ended` / `turn.ended` / `prompt.completed` when the
//! turn is over. A finished reply can show the same words in normal text.
use super::wire::{MainTurn, MainTurnCache};

/// Matches `footer_running_command_working`'s footer window, including the
/// Approved line and spinner tip that sit under a live Bash header.
const RUNNING_COMMAND_FOOTER_LINES: usize = 12;

/// The main turn decides when wire evidence is available. Without it, require
/// both the command suffix and the bold styling of Kimi's live Bash header.
pub(crate) fn running_command_text_is_live(
    turn: Option<MainTurn>,
    line_is_bold: Option<bool>,
    has_command_suffix: bool,
) -> bool {
    match turn {
        Some(MainTurn::Open) => true,
        Some(MainTurn::Settled) => false,
        None => has_command_suffix && line_is_bold == Some(true),
    }
}

/// True when the running-command line includes an actual `· $ command` suffix.
pub(crate) fn running_command_has_command_suffix(screen: &str) -> bool {
    let non_empty: Vec<&str> = screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = non_empty.len().saturating_sub(RUNNING_COMMAND_FOOTER_LINES);
    non_empty[start..].iter().any(|line| {
        is_running_command_line(line)
            && line.split_once('·').is_some_and(|(_, suffix)| {
                suffix
                    .trim_start()
                    .strip_prefix("$ ")
                    .is_some_and(|command| !command.trim().is_empty())
            })
    })
}

pub(crate) fn refresh_running_command_turn(
    home: Option<&std::path::Path>,
    cache: &mut MainTurnCache,
) -> Option<MainTurn> {
    let home = home?;
    super::wire::refresh_main_turn(home, cache).ok().flatten()
}

/// `Some` when a bottom status line is the running-command text.
/// `Some(true)` when that phrase is bold, matching Kimi's live header.
pub(crate) fn running_command_line_is_bold(ansi: &str) -> Option<bool> {
    let lines = styled_lines(ansi);
    let non_empty: Vec<&StyledLine> = lines
        .iter()
        .filter(|line| !line.plain.trim().is_empty())
        .collect();
    let start = non_empty.len().saturating_sub(RUNNING_COMMAND_FOOTER_LINES);
    let mut saw = false;
    let mut bold = false;
    for line in &non_empty[start..] {
        if !is_running_command_line(&line.plain) {
            continue;
        }
        saw = true;
        bold |= phrase_is_bold(line);
    }
    saw.then_some(bold)
}

struct StyledLine {
    plain: String,
    bold: Vec<bool>,
}

fn styled_lines(ansi: &str) -> Vec<StyledLine> {
    let mut lines = vec![empty_styled_line()];
    let mut bold = false;
    let mut chars = ansi.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            bold = consume_escape(&mut chars, bold);
            continue;
        }
        push_plain(&mut lines, ch, bold);
    }
    lines
}

fn empty_styled_line() -> StyledLine {
    StyledLine {
        plain: String::new(),
        bold: Vec::new(),
    }
}

fn push_plain(lines: &mut Vec<StyledLine>, ch: char, bold: bool) {
    if ch == '\n' {
        lines.push(empty_styled_line());
        return;
    }
    if ch == '\r' {
        return;
    }
    if let Some(line) = lines.last_mut() {
        line.plain.push(ch);
        line.bold.push(bold);
    }
}

fn consume_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, bold: bool) -> bool {
    match chars.peek().copied() {
        Some('[') => {
            chars.next();
            consume_csi(chars, bold)
        }
        Some(']') => {
            chars.next();
            consume_osc(chars);
            bold
        }
        _ => bold,
    }
}

fn consume_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, bold: bool) -> bool {
    let mut params = String::new();
    let final_byte = loop {
        match chars.next() {
            Some(next) if next.is_ascii_digit() || next == ';' => params.push(next),
            Some(next) => break Some(next),
            None => break None,
        }
    };
    if final_byte == Some('m') {
        sgr_bold(bold, &params)
    } else {
        bold
    }
}

fn consume_osc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(next) = chars.next() {
        if next == '\u{7}' {
            break;
        }
        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
            chars.next();
            break;
        }
    }
}

fn sgr_bold(current: bool, params: &str) -> bool {
    if params.is_empty() {
        return false;
    }
    let nums: Vec<u16> = params
        .split(';')
        .map(|param| param.parse().unwrap_or(0))
        .collect();
    let mut bold = current;
    let mut index = 0;
    while index < nums.len() {
        match nums[index] {
            0 => bold = false,
            1 => bold = true,
            22 => bold = false,
            38 | 48 => {
                if nums.get(index + 1) == Some(&2) {
                    index += 4;
                } else if nums.get(index + 1) == Some(&5) {
                    index += 2;
                }
            }
            _ => {}
        }
        index += 1;
    }
    bold
}

fn is_running_command_line(line: &str) -> bool {
    let trimmed = line.trim();
    let rest = if let Some(rest) = trimmed
        .strip_prefix('●')
        .or_else(|| trimmed.strip_prefix('•'))
    {
        if !rest.starts_with(char::is_whitespace) {
            return false;
        }
        rest.trim_start()
    } else {
        trimmed
    };
    let Some(after) = strip_prefix_ignore_ascii_case(rest, "running a command") else {
        return false;
    };
    let after = after.trim_start();
    after.is_empty() || after.starts_with('·')
}

fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let mut chars = text.chars();
    for expected in prefix.chars() {
        let actual = chars.next()?;
        if !actual.eq_ignore_ascii_case(&expected) {
            return None;
        }
    }
    Some(chars.as_str())
}

fn phrase_is_bold(line: &StyledLine) -> bool {
    let chars: Vec<char> = line.plain.chars().collect();
    let phrase: Vec<char> = "running a command".chars().collect();
    let Some(start) = chars.windows(phrase.len()).position(|window| {
        window
            .iter()
            .zip(&phrase)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    }) else {
        return false;
    };
    line.bold
        .get(start..start + phrase.len())
        .is_some_and(|flags| flags.iter().all(|flag| *flag))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY_FRAME: &str = "\
 ● Running a command\n\
╭──────────────────────────────────────╮\n\
│ >                                    │\n\
╰──────────────────────────────────────╯\n\
K3.1 Pro thinking: high  …/tmp/project  master\n\
                               context: 2% (17.9k/1M)";

    #[test]
    fn kimi_settled_turn_is_idle_even_when_the_reply_line_is_bold() {
        assert!(!running_command_text_is_live(
            Some(MainTurn::Settled),
            Some(true),
            false
        ));
        assert!(!running_command_text_is_live(
            Some(MainTurn::Settled),
            Some(false),
            true
        ));
        assert!(running_command_text_is_live(
            Some(MainTurn::Open),
            None,
            false
        ));
        assert!(!running_command_text_is_live(None, Some(true), false));
        assert!(!running_command_text_is_live(None, Some(false), false));
        assert!(!running_command_text_is_live(None, None, false));
        assert!(running_command_text_is_live(None, Some(true), true));
        assert!(!running_command_text_is_live(None, Some(false), true));
        assert!(!running_command_text_is_live(None, None, true));
    }

    #[test]
    fn kimi_plain_reply_line_is_not_the_bold_live_header() {
        assert_eq!(running_command_line_is_bold(REPLY_FRAME), Some(false));
        let live = REPLY_FRAME.replacen(
            "● Running a command",
            "● \u{1b}[1mRunning a command\u{1b}[0m",
            1,
        );
        assert_eq!(running_command_line_is_bold(&live), Some(true));
        let colored = REPLY_FRAME.replacen(
            "● Running a command",
            "● \u{1b}[1;38;2;1;2;3mRunning a command\u{1b}[0m",
            1,
        );
        assert_eq!(
            running_command_line_is_bold(&colored),
            Some(true),
            "an RGB channel of 1 must not be read as the bold attribute"
        );
        let color_only = REPLY_FRAME.replacen(
            "● Running a command",
            "● \u{1b}[38;2;1;2;3mRunning a command\u{1b}[0m",
            1,
        );
        assert_eq!(running_command_line_is_bold(&color_only), Some(false));
        assert!(
            !running_command_has_command_suffix(REPLY_FRAME),
            "a finished reply has no · command suffix"
        );
        let live_cmd = "\
 ● Running a command · $ sleep 45\n\
╭──────────────────────────────────────╮\n\
│ >                                    │\n\
╰──────────────────────────────────────╯\n\
K3.1 Pro thinking: high  …/tmp/project  master\n\
                               context: 2% (17.9k/1M)";
        assert!(running_command_has_command_suffix(live_cmd));
        for suffix in ["· explanation", "· $ ", "· $echo done"] {
            assert!(!running_command_has_command_suffix(&frame(&format!(
                "Running a command {suffix}"
            ))));
        }
    }

    fn frame(reply: &str) -> String {
        format!(" ● {reply}\n╭────────────────────────╮\n│ >                      │\n╰────────────────────────╯\nK3.1 Pro thinking: high  /tmp/project  master\ncontext: 2% (17.9k/1M)")
    }
    fn settled_home() -> std::path::PathBuf {
        let home = crate::utils::test_temp::unique_temp_path("kimi-activity-settled");
        let dir = home.join("sessions/wd_project/session_1");
        crate::platform::fs::private_dir(&dir.join("agents/main")).unwrap();
        let index =
            serde_json::json!({"sessionId":"session_1","sessionDir":dir,"workDir":"/tmp/project"});
        std::fs::write(home.join("session_index.jsonl"), format!("{index}\n")).unwrap();
        let rows = [
            serde_json::json!({"type":"turn.prompt","agentId":"main","turnId":1,"promptId":"p1","input":[{"type":"text","text":"Reply exactly: Running a command · $ echo done"}]}),
            serde_json::json!({"type":"agent.message.appended","message":{"message":{"role":"assistant","content":[{"type":"text","text":"Running a command · $ echo done"}],"toolCalls":[]},"meta":{"source":"llm"}}}),
            serde_json::json!({"type":"agent.turn.ended","turnId":1,"outcome":"done"}),
            serde_json::json!({"type":"turn.ended","agentId":"main","turnId":1,"reason":"completed"}),
            serde_json::json!({"type":"prompt.completed","agentId":"main","promptId":"p1","reason":"completed"}),
        ];
        std::fs::write(
            dir.join("agents/main/wire.jsonl"),
            rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        home
    }

    #[test]
    fn completed_wire_and_plain_command_shaped_final_are_not_live_work() {
        let home = settled_home();
        let text = std::fs::read_to_string(
            home.join("sessions/wd_project/session_1/agents/main/wire.jsonl"),
        )
        .unwrap();
        assert_eq!(
            super::super::wire::committed_final(
                &text,
                1,
                "Reply exactly: Running a command · $ echo done"
            ),
            Some("Running a command · $ echo done".to_owned())
        );
        let mut cache = super::super::wire::MainTurnCache::default();
        let turn = refresh_running_command_turn(Some(&home), &mut cache);
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(turn, Some(super::super::wire::MainTurn::Settled));
        let screen = frame("Running a command · $ echo done");
        let suffix = running_command_has_command_suffix(&screen);
        let bold = running_command_line_is_bold(&screen);
        assert_eq!(bold, Some(false));
        assert!(!running_command_text_is_live(turn,bold,suffix),
        "completed wire + plain final reply must be Idle, not Working; turn={turn:?} bold={bold:?} suffix={suffix}");
    }

    #[test]
    fn absent_wire_and_bold_final_reply_are_not_live_work() {
        let mut cache = super::super::wire::MainTurnCache::default();
        let turn = refresh_running_command_turn(None, &mut cache);
        let ansi = frame("\u{1b}[1mRunning a command\u{1b}[0m");
        let suffix = running_command_has_command_suffix(&ansi);
        let bold = running_command_line_is_bold(&ansi);
        assert_eq!(turn, None);
        assert_eq!(bold, Some(true));
        assert!(
            !running_command_text_is_live(turn, bold, suffix),
            "finished Markdown-bold reply with no wire must not be treated as live tool work"
        );
    }

    #[test]
    fn absent_wire_and_non_command_middle_dot_reply_are_not_live_work() {
        let screen = frame("Running a command · explanation");
        let suffix = running_command_has_command_suffix(&screen);
        let bold = running_command_line_is_bold(&screen);
        assert_eq!(bold, Some(false));
        assert!(
            !running_command_text_is_live(None, bold, suffix),
            "a middle-dot explanation is not a · $ command header; suffix={suffix}"
        );
    }
}
