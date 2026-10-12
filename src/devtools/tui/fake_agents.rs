//! Fake provider CLIs for driver sessions. The scratch Bus finds them first on
//! its PATH, so adding a `claude`, `codex` or `cursor` agent never starts a
//! real provider or spends model usage.
//!
//! Each fake is the `bus` binary itself under the provider's name, so Bus sees
//! a process called `claude` (or `codex`, `cursor-agent`). The fake Claude
//! fires Bus's own hooks from the `--settings` file Bus passes it and draws a
//! Claude-like prompt box. The fake Codex fires the hooks Bus writes to the
//! project's `.codex/hooks.json`, draws Codex's `›` composer and, like Codex,
//! starts its session with the first prompt. Both answer every prompt with
//! `fake reply: <prompt>`.
use crate::agents::providers::claude_code::hooks::claude_lifted_images;
use ratatui::buffer::CellWidth;
use ratatui::style::Style;
use ratatui::text::Span;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Set in the scratch Bus's environment; the fake only runs where it is set.
pub(super) const FAKE_AGENT_ENV: &str = "BUS_TUI_FAKE_AGENT";
const PROVIDER_NAMES: &[&str] = &["claude", "codex", "cursor-agent"];

/// Links each provider name, and each effect stand-in, in `bin_dir` to the
/// `bus` binary.
pub(super) fn install(bin_dir: &Path, binary: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(bin_dir)?;
    for name in PROVIDER_NAMES.iter().chain(super::effects::COMMANDS) {
        let link = bin_dir.join(exe_name(name));
        let _ = std::fs::remove_file(&link);
        link_or_copy(binary, &link)?;
    }
    Ok(())
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

#[cfg(unix)]
fn link_or_copy(binary: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(binary, link)
}

#[cfg(not(unix))]
fn link_or_copy(binary: &Path, link: &Path) -> std::io::Result<()> {
    std::fs::hard_link(binary, link).or_else(|_| std::fs::copy(binary, link).map(|_| ()))
}

/// The provider a fake should play, when this process is one: invoked under a
/// provider's name inside a driver session.
pub(super) fn fake_provider(argv0: &str) -> Option<&'static str> {
    stand_in(argv0, PROVIDER_NAMES)
}

/// The name of `names` this process runs under, inside a driver session only.
pub(super) fn stand_in(argv0: &str, names: &[&'static str]) -> Option<&'static str> {
    std::env::var_os(FAKE_AGENT_ENV)?;
    let name = Path::new(argv0).file_stem()?.to_str()?;
    names.iter().copied().find(|candidate| *candidate == name)
}

pub(super) fn run(provider: &str, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--version" || a == "-v") {
        let _ = writeln!(std::io::stdout(), "0.0.0 (bus tui fake {provider})");
        return 0;
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let hooks = hooks_path(provider, args, &cwd)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|settings| settings["hooks"].clone())
        .unwrap_or(Value::Null);
    let mut agent = FakeAgent::new(provider, hooks);
    // Codex defers SessionStart to its first prompt; Bus delivers to it unbound.
    if !agent.codex() {
        agent.session_start();
    }
    agent.draw_prompt("");
    if crossterm::terminal::enable_raw_mode().is_err() {
        return 1;
    }
    let code = agent.read_loop();
    let _ = crossterm::terminal::disable_raw_mode();
    code
}

/// Where the provider being played reads Bus's hooks: Claude from the
/// `--settings` file Bus passes it, Codex from the nearest `.codex/hooks.json`
/// at or above its directory, which Bus writes there before launching it.
pub(super) fn hooks_path(provider: &str, args: &[String], cwd: &Path) -> Option<PathBuf> {
    if provider == "codex" {
        cwd.ancestors()
            .map(|dir| dir.join(".codex/hooks.json"))
            .find(|path| path.is_file())
    } else {
        settings_path(args)
    }
}

fn settings_path(args: &[String]) -> Option<PathBuf> {
    args.iter()
        .position(|a| a == "--settings")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
}

struct FakeAgent {
    provider: String,
    hooks: Value,
    session: String,
    turns: u64,
    /// Images attached so far; Claude numbers `[Image #N]` across a session.
    images: usize,
}

impl FakeAgent {
    fn new(provider: &str, hooks: Value) -> Self {
        Self {
            provider: provider.to_owned(),
            hooks,
            session: pseudo_uuid(),
            turns: 0,
            images: 0,
        }
    }

    fn codex(&self) -> bool {
        self.provider == "codex"
    }

    /// `payload` for `event` as the played provider reports it. Codex names
    /// its turn `turn_id` (Claude: `prompt_id`) and always names its rollout:
    /// Bus ignores a Codex hook without one as a background session. The path
    /// is no `rollout-*` file, so Bus reads no usage from it.
    fn payload(&self, event: &str, turn: Option<&str>, mut payload: Value) -> Value {
        payload["hook_event_name"] = json!(event);
        payload["session_id"] = json!(self.session);
        payload["cwd"] = json!(std::env::current_dir().ok());
        if let Some(turn) = turn {
            payload[if self.codex() { "turn_id" } else { "prompt_id" }] = json!(turn);
        }
        if self.codex() {
            let rollout = std::env::temp_dir().join(format!("fake-codex-{}.jsonl", self.session));
            payload["transcript_path"] = json!(rollout);
        }
        payload
    }

    /// Runs Bus's hook command for `event` with its payload on stdin.
    fn hook(&self, event: &str, turn: Option<&str>, payload: Value) {
        let Some(command) = self.hooks[event][0]["hooks"][0]["command"].as_str() else {
            return;
        };
        let payload = self.payload(event, turn, payload);
        let shell = if cfg!(windows) {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };
        let Ok(mut child) = Command::new(shell.0)
            .args([shell.1, command])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.to_string().as_bytes());
        }
        let _ = child.wait();
    }

    fn session_start(&self) {
        self.hook("SessionStart", None, json!({"source": "startup"}));
    }

    /// Draws an empty prompt under the transcript: Claude's box opens with a
    /// rule, Codex's composer with a blank row.
    fn draw_prompt(&self, draft: &str) {
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", prompt_top(&self.provider));
        let _ = out.flush();
        self.redraw(draft);
    }

    /// The draft as the played provider shows and submits it, with the images
    /// it lifted: Claude lifts image lines into chips, Codex keeps them as text.
    fn lift(&self, draft: &str) -> (String, usize) {
        if self.codex() {
            (draft.to_owned(), 0)
        } else {
            lift_images(draft, self.images)
        }
    }

    fn redraw(&self, draft: &str) {
        let (shown, _) = self.lift(draft);
        let cols = crossterm::terminal::size().map_or(80, |(cols, _)| cols);
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", prompt_box(&shown, &self.provider, cols));
        let _ = out.flush();
    }

    fn submit(&mut self, prompt: &str) {
        self.turns += 1;
        if self.codex() && self.turns == 1 {
            self.session_start();
        }
        let turn = format!("{}-{}", self.session, self.turns);
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", submitted(prompt));
        let _ = out.flush();
        self.hook("UserPromptSubmit", Some(&turn), json!({"prompt": prompt}));
        if let Some(seconds) = work_seconds(prompt) {
            spin(seconds, self.codex());
        }
        let reply = format!("fake reply: {}", prompt.trim());
        let bullet = if self.codex() { '•' } else { '⏺' };
        let _ = write!(out, "{bullet} {}\r\n", reply.replace('\n', "\r\n  "));
        let _ = out.flush();
        self.hook(
            "Stop",
            Some(&turn),
            json!({"last_assistant_message": reply, "stop_hook_active": false}),
        );
        self.draw_prompt("");
    }

    fn read_loop(&mut self) -> i32 {
        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 4096];
        let mut pending = Vec::new();
        let mut draft = String::new();
        loop {
            let count = match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => return 0,
                Ok(count) => count,
            };
            pending.extend_from_slice(&buffer[..count]);

            for event in drain_input(&mut pending) {
                match event {
                    Input::Text(text) => {
                        draft.push_str(&text);
                        self.redraw(&draft);
                    }
                    Input::Backspace => {
                        if draft.pop().is_some() {
                            self.redraw(&draft);
                        }
                    }
                    Input::KillLine => {
                        kill_line(&mut draft);
                        self.redraw(&draft);
                    }
                    Input::Enter if !draft.trim().is_empty() => {
                        let (prompt, images) = self.lift(&std::mem::take(&mut draft));
                        self.images += images;
                        self.submit(&prompt);
                    }
                    Input::Enter => {}
                    Input::Interrupt => return 0,
                }
            }
        }
    }
}

/// Empties the draft's last line, or when it is empty already, joins it to
/// the line before, so repeated kills clear a multi-line draft.
pub(super) fn kill_line(draft: &mut String) {
    match draft.rfind('\n') {
        Some(at) if at + 1 == draft.len() => {
            draft.pop();
        }
        Some(at) => draft.truncate(at + 1),
        None => draft.clear(),
    }
}

/// What opens a new prompt below the transcript, from the start of a line:
/// a blank row and a rule for Claude Code, one blank row for Codex.
pub(super) fn prompt_top(provider: &str) -> String {
    if provider == "codex" {
        "\r\n".into()
    } else {
        format!("\r\n{}\r\n", "─".repeat(60))
    }
}

/// Codex's empty composer text, one Bus reads as an empty composer.
const CODEX_PLACEHOLDER: &str = "Ask Codex to do anything";

/// The prompt box redrawn in place the way the played provider shows a
/// draft, continuation lines indented under its marker. Claude Code: `❯` over
/// a rule and its hint, image lines already lifted into leading `[Image #N]`
/// chips. Codex: a `›` composer with a dim placeholder when empty, then a
/// blank row and its model line, which is where Bus stops reading the draft.
/// It starts and ends with the cursor on the box's first row: moving relative
/// to the cursor, never to a saved position, keeps the box in place when
/// drawing it scrolls a full pane. The move counts the rows the box takes on
/// a `cols`-wide pane, so lines that wrap count too.
pub(super) fn prompt_box(shown: &str, provider: &str, cols: u16) -> String {
    let codex = provider == "codex";
    let (marker, footer) = if codex {
        (
            '›',
            [String::new(), format!("  gpt-fake · fake {provider}")],
        )
    } else {
        (
            '❯',
            [
                "─".repeat(60),
                format!("  ? for shortcuts · fake {provider}"),
            ],
        )
    };
    let placeholder = codex && shown.is_empty();
    let lines = if placeholder {
        CODEX_PLACEHOLDER
    } else {
        shown
    };
    let rows = |text: &str| screen_rows(text, cols);
    let draft_rows: usize = lines
        .split('\n')
        .map(|line| rows(&format!("{marker} {line}")))
        .sum();
    let text = if placeholder {
        format!("\x1b[2m{CODEX_PLACEHOLDER}\x1b[22m")
    } else {
        shown.replace('\n', "\r\n  ")
    };
    format!(
        "\r\x1b[J{marker} {text}\r\n{}\x1b[{}A\r\x1b[{}C",
        footer.join("\r\n"),
        draft_rows + footer.iter().map(|line| rows(line)).sum::<usize>() - 1,
        2 + shown.rsplit('\n').next().unwrap_or("").chars().count()
    )
}

/// The rows `text` takes on a `cols`-wide pane, counted the way a terminal
/// wraps: by whole grapheme clusters (a joined emoji is one two-cell
/// cluster), and a wide one that would start in the last column leaves it
/// blank and wraps instead of splitting.
pub(super) fn screen_rows(text: &str, cols: u16) -> usize {
    let cols = usize::from(cols.max(1));
    let (mut rows, mut col) = (1, 0);
    for grapheme in Span::raw(text).styled_graphemes(Style::default()) {
        let width = usize::from(grapheme.symbol.cell_width());
        if col + width > cols {
            rows += 1;
            col = 0;
        }
        col += width;
    }
    rows
}

/// Replaces the box (the cursor is on its first row) with the submitted
/// prompt, as Claude Code does. The fake Codex marks it the same way, not with
/// Codex's `›`: it shows no composer while it answers, so a `›` here would be
/// the last one on screen and Bus would read the sent prompt as a draft.
pub(super) fn submitted(prompt: &str) -> String {
    format!("\r\x1b[J> {}\r\n", prompt.replace('\n', "\r\n  "))
}

/// The draft as Claude Code submits it: lines Bus expects Claude to lift as
/// images become leading `[Image #N]` chips (numbered after the session's
/// earlier images), followed by the other non-blank lines. Returns that prompt
/// and how many images it lifted.
pub(super) fn lift_images(draft: &str, earlier: usize) -> (String, usize) {
    let (images, rest) = claude_lifted_images(draft);
    if images == 0 {
        return (draft.to_owned(), 0);
    }
    let chips: String = (1..=images)
        .map(|n| format!("[Image #{}]", earlier + n))
        .collect();
    (format!("{chips}{rest}"), images)
}

/// A prompt containing `fake:work N` keeps the fake busy for N seconds (at
/// most 600), so a driver can see a Working agent and a live spinner.
fn work_seconds(prompt: &str) -> Option<u64> {
    let rest = prompt.split("fake:work").nth(1)?;
    let seconds: u64 = rest.split_whitespace().next()?.parse().ok()?;
    Some(seconds.min(600))
}

/// The working line, redrawn in place: Claude's `✻ Working… (3s · esc to
/// interrupt)`, or Codex's `• Working (3s • esc to interrupt)`.
fn spin(seconds: u64, codex: bool) {
    let started = std::time::Instant::now();
    let mut out = std::io::stdout();
    let mut index = 0;
    while started.elapsed().as_secs() < seconds {
        let _ = write!(
            out,
            "\r\x1b[2K{}",
            working_line(codex, index, started.elapsed().as_secs())
        );
        let _ = out.flush();
        index += 1;
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    let _ = write!(out, "\r\x1b[2K");
    let _ = out.flush();
}

pub(super) fn working_line(codex: bool, frame: usize, seconds: u64) -> String {
    if codex {
        let bullet = ['•', '◦'][frame % 2];
        format!("{bullet} Working ({seconds}s • esc to interrupt)")
    } else {
        let spinner = ['✻', '✶', '✢', '·'][frame % 4];
        format!("{spinner} Working… ({seconds}s · esc to interrupt)")
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Input {
    Text(String),
    Backspace,
    /// Ctrl+K or Ctrl+U.
    KillLine,
    Enter,
    Interrupt,
}

/// Splits raw terminal input into text, Enter, Backspace and Ctrl+C/D,
/// unwrapping bracketed pastes and dropping other escape sequences. Bytes of
/// an unfinished sequence stay in `pending`.
fn drain_input(pending: &mut Vec<u8>) -> Vec<Input> {
    let mut events = Vec::new();
    let mut text = Vec::new();
    let mut i = 0;
    let flush = |text: &mut Vec<u8>, events: &mut Vec<Input>| {
        if !text.is_empty() {
            events.push(Input::Text(String::from_utf8_lossy(text).into_owned()));
            text.clear();
        }
    };
    while i < pending.len() {
        let rest = &pending[i..];
        if rest.starts_with(b"\x1b[200~") {
            let Some(end) = find(rest, b"\x1b[201~") else {
                break;
            };
            text.extend_from_slice(&rest[6..end]);
            i += end + 6;
            continue;
        }
        match rest[0] {
            // Claude Code submits on CR; a typed LF (Ctrl+J) is a newline.
            b'\r' => {
                flush(&mut text, &mut events);
                events.push(Input::Enter);
                i += 1;
            }
            b'\n' => {
                text.push(b'\n');
                i += 1;
            }
            0x7f | 0x08 => {
                flush(&mut text, &mut events);
                events.push(Input::Backspace);
                i += 1;
            }
            0x03 | 0x04 => {
                flush(&mut text, &mut events);
                events.push(Input::Interrupt);
                i += 1;
            }
            // Ctrl+K and Ctrl+U: Bus withdraws a draft with them.
            0x0b | 0x15 => {
                flush(&mut text, &mut events);
                events.push(Input::KillLine);
                i += 1;
            }
            0x1b => {
                // CSI sequences end at a byte in 0x40..=0x7e; `CSI 13 u` is Enter.
                if rest.len() < 2 {
                    break;
                }
                if rest[1] != b'[' {
                    i += 2;
                    continue;
                }
                let Some(end) = rest[2..].iter().position(|b| (0x40..=0x7e).contains(b)) else {
                    break;
                };
                let sequence = &rest[..end + 3];
                if sequence == b"\x1b[13u" {
                    flush(&mut text, &mut events);
                    events.push(Input::Enter);
                }
                i += end + 3;
            }
            byte if byte < 0x20 && byte != b'\t' => i += 1,
            _ => {
                text.push(rest[0]);
                i += 1;
            }
        }
    }
    // A character split across reads waits in `pending` for its last bytes.
    if i == pending.len() {
        let split = text.len() - super::trace::complete_utf8_len(&text);
        text.truncate(text.len() - split);
        i -= split;
    }
    flush(&mut text, &mut events);
    pending.drain(..i);
    events
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn pseudo_uuid() -> String {
    use sha2::Digest as _;
    let seed = format!("{:?}-{}", std::time::SystemTime::now(), std::process::id());
    let hex: String = sha2::Sha256::digest(seed.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

#[cfg(test)]
#[path = "tests/fake_agents_test.rs"]
mod tests;
