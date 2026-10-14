//! Fake provider CLIs for driver sessions. The scratch Bus finds them first on
//! its PATH, so adding a `claude`, `codex` or `cursor` agent never starts a
//! real provider or spends model usage.
//!
//! Each fake is the `bus` binary itself under the provider's name, so Bus sees
//! a process called `claude` (or `codex`, `cursor-agent`). The fake Claude
//! fires Bus's own hooks from the `--settings` file Bus passes it and draws a
//! Claude-like prompt box. The fake Codex fires the hooks Bus writes to the
//! project's `.codex/hooks.json`, draws Codex's `›` composer and, like Codex,
//! starts its session with the first prompt. The fake Cursor fires the hooks
//! Bus writes to the project's `.cursor/hooks.json` and draws Cursor's `→`
//! composer. All answer every prompt with `fake reply: <prompt>`.
mod dialogs;
mod directives;
mod question;

use crate::agents::providers::claude_code::hooks::claude_lifted_images;
use directives::{api_error, dialog_seconds, show_dialog, work_seconds};
use ratatui::buffer::CellWidth;
use ratatui::style::Style;
use ratatui::text::Span;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

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
    // Claude and Cursor start theirs at launch: Bus delivers only to a bound session.
    if !agent.codex() {
        agent.session_start();
    }
    agent.draw_prompt();
    if crossterm::terminal::enable_raw_mode().is_err() {
        return 1;
    }
    agent.input = Some(read_input());
    let code = agent.read_loop();
    let _ = crossterm::terminal::disable_raw_mode();
    code
}

/// Where the provider being played reads Bus's hooks: Claude from the
/// `--settings` file Bus passes it, Codex and Cursor from the nearest
/// `.codex/hooks.json` or `.cursor/hooks.json` at or above its directory,
/// which Bus writes there before launching it.
fn hooks_path(provider: &str, args: &[String], cwd: &Path) -> Option<PathBuf> {
    let file = match provider {
        "codex" => ".codex/hooks.json",
        "cursor-agent" => ".cursor/hooks.json",
        _ => return settings_path(args),
    };
    cwd.ancestors()
        .map(|dir| dir.join(file))
        .find(|path| path.is_file())
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
    /// Terminal input, read on its own thread so a live turn can take typing.
    input: Option<mpsc::Receiver<Vec<u8>>>,
    /// Bytes of an unfinished input sequence.
    pending: Vec<u8>,
    draft: String,
    /// While a live turn runs: when it started and its spinner frame.
    working: Option<(Instant, usize)>,
}

/// What a keystroke ended in, besides editing the draft.
enum Typed {
    Submit(String),
    /// The provider's stop key during a turn: Esc for Claude Code and
    /// Codex, Ctrl+C for Cursor.
    Stop,
    Exit,
}

impl FakeAgent {
    fn new(provider: &str, hooks: Value) -> Self {
        Self {
            provider: provider.to_owned(),
            hooks,
            session: pseudo_uuid(),
            turns: 0,
            images: 0,
            input: None,
            pending: Vec::new(),
            draft: String::new(),
            working: None,
        }
    }

    fn codex(&self) -> bool {
        self.provider == "codex"
    }

    fn cursor(&self) -> bool {
        self.provider == "cursor-agent"
    }

    /// Codex and Cursor keep a live composer under their working line, so a
    /// message typed meanwhile joins the running turn; Claude hides its box.
    fn live_turn(&self) -> bool {
        self.codex() || self.cursor()
    }

    /// `payload` for `event` as the played provider reports it. Codex names
    /// its turn `turn_id` (Claude: `prompt_id`) and always names its rollout:
    /// Bus ignores a Codex hook without one as a background session. The path
    /// is no `rollout-*` file, so Bus reads no usage from it. Cursor names its
    /// session `conversation_id` and its turn `generation_id`, and gets no
    /// transcript: Bus would wait for one to settle the reply.
    fn payload(&self, event: &str, turn: Option<&str>, mut payload: Value) -> Value {
        payload["hook_event_name"] = json!(event);
        let (session, turn_key) = match self.provider.as_str() {
            "codex" => ("session_id", "turn_id"),
            "cursor-agent" => ("conversation_id", "generation_id"),
            _ => ("session_id", "prompt_id"),
        };
        payload[session] = json!(self.session);
        payload["cwd"] = json!(std::env::current_dir().ok());
        if let Some(turn) = turn {
            payload[turn_key] = json!(turn);
        }
        if self.codex() {
            let rollout = std::env::temp_dir().join(format!("fake-codex-{}.jsonl", self.session));
            payload["transcript_path"] = json!(rollout);
        }
        payload
    }

    /// Runs every command hook for `event`, in file order, with its payload on
    /// stdin. A project's `.codex/hooks.json` keeps its own hooks before the
    /// one Bus appends, and Codex runs them all; Claude's `--settings` holds
    /// only Bus's. Claude and Codex nest commands in an entry's `hooks`;
    /// Cursor's entry is the command itself.
    fn hook(&self, event: &str, turn: Option<&str>, payload: Value) {
        let payload = self.payload(event, turn, payload).to_string();
        let commands = self.hooks[event]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|entry| match entry["hooks"].as_array() {
                Some(hooks) => hooks.iter().collect(),
                None => vec![entry],
            })
            .filter_map(|hook| hook["command"].as_str());
        for command in commands {
            run_hook(command, &payload);
        }
    }

    fn session_start(&self) {
        let event = if self.cursor() {
            "sessionStart"
        } else {
            "SessionStart"
        };
        self.hook(event, None, json!({"source": "startup"}));
    }

    /// Draws the prompt under the transcript: Claude's box opens with a rule,
    /// Codex's composer with a blank row, Cursor's with a half-block border.
    fn draw_prompt(&self) {
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", prompt_top(&self.provider));
        let _ = out.flush();
        self.redraw();
    }

    /// The draft as the played provider shows and submits it, with the images
    /// it lifted: Claude lifts image lines into chips, Codex and Cursor keep
    /// them as text.
    fn lift(&self, draft: &str) -> (String, usize) {
        if self.live_turn() {
            (draft.to_owned(), 0)
        } else {
            lift_images(draft, self.images)
        }
    }

    /// Redraws the prompt in place, under the working line while a turn runs.
    fn redraw(&self) {
        let (shown, _) = self.lift(&self.draft);
        let cols = crossterm::terminal::size().map_or(80, |(cols, _)| cols);
        let frame = match self.working {
            Some((started, frame)) => {
                let line = working_line(&self.provider, frame, started.elapsed().as_secs());
                working_block(&line, &shown, &self.provider, true, cols)
            }
            None => prompt_box(&shown, &self.provider, cols),
        };
        let mut out = std::io::stdout();
        let _ = write!(out, "{frame}");
        let _ = out.flush();
    }

    /// Applies one input to the draft, redrawing it.
    fn apply(&mut self, input: Input) -> Option<Typed> {
        match input {
            Input::Text(text) => self.draft.push_str(&text),
            // Nothing to erase: nothing to redraw.
            Input::Backspace => {
                self.draft.pop()?;
            }
            Input::KillLine => kill_line(&mut self.draft),
            Input::Enter if !self.draft.trim().is_empty() => {
                let draft = std::mem::take(&mut self.draft);
                let (prompt, images) = self.lift(&draft);
                self.images += images;
                return Some(Typed::Submit(prompt));
            }
            Input::Enter => return None,
            Input::Escape if self.working.is_some() && !self.cursor() => return Some(Typed::Stop),
            Input::Escape => return None,
            Input::Interrupt if self.working.is_some() && self.cursor() => {
                return Some(Typed::Stop)
            }
            Input::Interrupt => return Some(Typed::Exit),
        }
        self.redraw();
        None
    }

    /// Answers `prompt` in one turn; the exit code when interrupted meanwhile.
    fn submit(&mut self, prompt: &str) -> Option<i32> {
        self.turns += 1;
        let turn = format!("{}-{}", self.session, self.turns);
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", submitted(prompt));
        let _ = out.flush();
        let mut prompts = vec![prompt.trim().to_owned()];
        if self.live_turn() {
            // Codex and Cursor clear the composer and show their working line
            // at once, before any hook runs: Bus confirms the submit from that
            // screen.
            self.begin_turn();
            if self.codex() && self.turns == 1 {
                self.session_start();
            }
        }
        let submit = if self.cursor() {
            "beforeSubmitPrompt"
        } else {
            "UserPromptSubmit"
        };
        self.hook(submit, Some(&turn), json!({"prompt": prompt}));
        if dialogs::redraw_requested(prompt) {
            if let Err(code) = self.settlement_dialog() {
                return Some(code);
            }
        }
        if let Some(seconds) = dialog_seconds(prompt) {
            show_dialog(seconds);
        }
        if question::requested(&self.provider, prompt) {
            if let Err(code) = self.queued_question() {
                return Some(code);
            }
        }
        let mut stopped = false;
        match work_seconds(prompt) {
            Some(seconds) if self.live_turn() => match self.work(seconds) {
                Ok(Turn::Done(steered)) => {
                    prompts.extend(steered.iter().map(|p| p.trim().to_owned()));
                }
                Ok(Turn::Stopped(steered)) => {
                    prompts.extend(steered.iter().map(|p| p.trim().to_owned()));
                    stopped = true;
                }
                Err(code) => return Some(code),
            },
            Some(seconds) => stopped = self.spin(seconds),
            None => {}
        }
        if self.live_turn() {
            self.end_turn(&prompts[1..]);
        }
        if stopped {
            // No provider reports an interrupted turn with a hook.
            self.stop_turn(prompt);
            return None;
        }
        if let Some(payload) = api_error(&self.provider, prompt) {
            let text = payload["last_assistant_message"]
                .as_str()
                .unwrap_or_default();
            let _ = write!(out, "  ⎿  {text}\r\n");
            let _ = out.flush();
            self.hook("StopFailure", Some(&turn), payload);
            self.draw_prompt();
            return None;
        }
        let reply = format!("fake reply: {}", prompts.join("\n"));
        let bullet = if self.codex() { '•' } else { '⏺' };
        let _ = write!(out, "{bullet} {}\r\n", reply.replace('\n', "\r\n  "));
        let _ = out.flush();
        if self.cursor() {
            // Cursor reports the reply, then a completed stop that Bus settles
            // it on.
            self.hook("afterAgentResponse", Some(&turn), json!({"text": reply}));
            self.hook("stop", Some(&turn), json!({"status": "completed"}));
        } else {
            self.hook(
                "Stop",
                Some(&turn),
                json!({"last_assistant_message": reply, "stop_hook_active": false}),
            );
        }
        self.draw_prompt();
        None
    }

    /// Starts a live turn below the sent prompt: the working line over a
    /// live composer, as real Codex and Cursor show every turn.
    fn begin_turn(&mut self) {
        let cols = crossterm::terminal::size().map_or(80, |(cols, _)| cols);
        let (shown, _) = self.lift(&self.draft);
        let mut out = std::io::stdout();
        let line = working_line(&self.provider, 0, 0);
        let block = working_block(&line, &shown, &self.provider, false, cols);
        let _ = write!(out, "{block}");
        let _ = out.flush();
        self.working = Some((Instant::now(), 0));
    }

    /// Claude's working line, redrawn in place for `seconds` while its box
    /// is hidden. Typed input waits for the turn to end, except Esc, which
    /// stops it: returns whether it did.
    fn spin(&mut self, seconds: u64) -> bool {
        let started = Instant::now();
        let mut out = std::io::stdout();
        let mut frame = 0;
        let mut stopped = false;
        while started.elapsed().as_secs() < seconds {
            let _ = write!(
                out,
                "\r\x1b[2K{}",
                working_line(&self.provider, frame, started.elapsed().as_secs())
            );
            let _ = out.flush();
            frame += 1;
            let tick = std::time::Duration::from_millis(120);
            match self.input.as_ref().map(|input| input.recv_timeout(tick)) {
                // Bus writes a key in one piece; a lone Esc is the key.
                Some(Ok(bytes)) if bytes == b"\x1b" => {
                    stopped = true;
                    break;
                }
                Some(Ok(bytes)) => self.pending.extend_from_slice(&bytes),
                Some(Err(mpsc::RecvTimeoutError::Timeout)) => {}
                _ => std::thread::sleep(tick),
            }
        }
        let _ = write!(out, "\r\x1b[2K");
        let _ = out.flush();
        stopped
    }

    /// Ends a turn the stop key interrupted, without a reply or a hook, as
    /// each provider does: Codex notes it in the transcript, Claude Code puts
    /// the prompt back into its box, Cursor returns to its composer.
    fn stop_turn(&mut self, prompt: &str) {
        let mut out = std::io::stdout();
        if self.codex() {
            let _ = write!(out, "{CODEX_INTERRUPTED}\r\n");
        } else if !self.cursor() {
            self.draft = prompt.to_owned();
        }
        let _ = out.flush();
        self.draw_prompt();
    }

    /// Keeps a live turn working until `seconds` after it began, reading
    /// input, so Bus can type a steering message into the running turn. Each
    /// prompt entered meanwhile joins the turn, whose one reply answers it
    /// too. Returns those prompts and whether the stop key ended the turn
    /// early, or the exit code when the provider was quit.
    fn work(&mut self, seconds: u64) -> Result<Turn, i32> {
        let mut steered = Vec::new();
        while let Some((started, frame)) = self.working {
            if started.elapsed().as_secs() >= seconds {
                break;
            }
            let tick = std::time::Duration::from_millis(120);
            let bytes = match self.input.as_ref().map(|input| input.recv_timeout(tick)) {
                Some(Ok(bytes)) => bytes,
                Some(Err(mpsc::RecvTimeoutError::Timeout)) => Vec::new(),
                _ => {
                    std::thread::sleep(tick);
                    Vec::new()
                }
            };
            self.pending.extend_from_slice(&bytes);
            for input in drain_input(&mut self.pending) {
                match self.apply(input) {
                    Some(Typed::Submit(prompt)) => steered.push(prompt),
                    Some(Typed::Stop) => return Ok(Turn::Stopped(steered)),
                    Some(Typed::Exit) => return Err(0),
                    None => {}
                }
            }
            self.working = Some((started, frame + 1));
            self.redraw();
        }
        Ok(Turn::Done(steered))
    }

    /// Ends a live turn: its working block goes, and the prompts `steered`
    /// into it join the transcript before the reply.
    fn end_turn(&mut self, steered: &[String]) {
        let cols = crossterm::terminal::size().map_or(80, |(cols, _)| cols);
        let seconds = self
            .working
            .map_or(0, |(started, _)| started.elapsed().as_secs());
        self.working = None;
        let mut out = std::io::stdout();
        let line = working_line(&self.provider, 0, seconds);
        let _ = write!(out, "{}", work_done(&line, &self.provider, cols));
        for prompt in steered {
            let _ = write!(out, "{}", submitted(prompt));
        }
        let _ = out.flush();
    }

    fn read_loop(&mut self) -> i32 {
        loop {
            // Input typed during a Claude turn waited in `pending`.
            let inputs = drain_input(&mut self.pending);
            if inputs.is_empty() {
                let Some(Ok(bytes)) = self.input.as_ref().map(|input| input.recv()) else {
                    return 0;
                };
                self.pending.extend_from_slice(&bytes);
                continue;
            }
            for input in inputs {
                match self.apply(input) {
                    Some(Typed::Submit(prompt)) => {
                        if let Some(code) = self.submit(&prompt) {
                            return code;
                        }
                    }
                    Some(Typed::Exit) => return 0,
                    Some(Typed::Stop) | None => {}
                }
            }
        }
    }
}

/// Runs one hook command with `payload` on its stdin and waits for it.
fn run_hook(command: &str, payload: &str) {
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
        let _ = stdin.write_all(payload.as_bytes());
    }
    let _ = child.wait();
}

/// Terminal input, read on its own thread until stdin closes.
fn read_input() -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 4096];
        while let Ok(count) = stdin.read(&mut buffer) {
            if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                break;
            }
        }
    });
    receiver
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
/// a blank row and a rule for Claude Code, one blank row for Codex, a blank
/// row and the composer's top border for Cursor.
fn prompt_top(provider: &str) -> String {
    match provider {
        "codex" => "\r\n".into(),
        _ => format!("\r\n{}", composer_top(provider)),
    }
}

/// The line over the composer itself: Claude's rule, Cursor's half-block
/// border; Codex draws none.
fn composer_top(provider: &str) -> String {
    match provider {
        "codex" => String::new(),
        "cursor-agent" => format!(" {}\r\n", "▄".repeat(60)),
        _ => format!("{}\r\n", "─".repeat(60)),
    }
}

/// How a live turn ended: on its own, or stopped by the stop key; either way
/// with the prompts typed into it.
enum Turn {
    Done(Vec<String>),
    Stopped(Vec<String>),
}

/// What Codex 0.162 prints when Esc interrupts a turn (live capture).
const CODEX_INTERRUPTED: &str =
    "■ Conversation interrupted - use /feedback if something went wrong";

/// Codex's empty composer text, one Bus reads as an empty composer.
const CODEX_PLACEHOLDER: &str = "Ask Codex to do anything";
/// Cursor's empty composer text when idle, and while a turn runs; Bus reads
/// both as an empty composer.
const CURSOR_PLACEHOLDER: &str = "Plan, search, build anything";
const CURSOR_FOLLOW_UP: &str = "Add a follow-up";

/// The prompt box redrawn in place the way the played provider shows a
/// draft, continuation lines indented under its marker. Claude Code: `❯` over
/// a rule and its hint, image lines already lifted into leading `[Image #N]`
/// chips. Codex: a `›` composer with a dim placeholder when empty, then a
/// blank row and its model line, which is where Bus stops reading the draft.
/// Cursor: a `→` composer with a dim placeholder when empty, then its
/// half-block bottom border, where Bus stops reading, and its model line.
/// It starts and ends with the cursor on the box's first row: moving relative
/// to the cursor, never to a saved position, keeps the box in place when
/// drawing it scrolls a full pane. The move counts the rows the box takes on
/// a `cols`-wide pane, so lines that wrap count too.
pub(super) fn prompt_box(shown: &str, provider: &str, cols: u16) -> String {
    prompt_box_with(shown, provider, CURSOR_PLACEHOLDER, cols)
}

/// [`prompt_box`], with Cursor's empty composer showing `cursor_placeholder`.
fn prompt_box_with(shown: &str, provider: &str, cursor_placeholder: &str, cols: u16) -> String {
    let (marker, footer, empty) = match provider {
        "codex" => (
            '›',
            [String::new(), format!("  gpt-fake · fake {provider}")],
            Some(CODEX_PLACEHOLDER),
        ),
        "cursor-agent" => (
            '→',
            [
                format!(" {}", "▀".repeat(60)),
                format!("  fake-model · fake {provider}"),
            ],
            Some(cursor_placeholder),
        ),
        _ => (
            '❯',
            [
                "─".repeat(60),
                format!("  ? for shortcuts · fake {provider}"),
            ],
            None,
        ),
    };
    let placeholder = empty.filter(|_| shown.is_empty());
    let lines = placeholder.unwrap_or(shown);
    let rows = |text: &str| screen_rows(text, cols);
    let draft_rows: usize = lines
        .split('\n')
        .map(|line| rows(&format!("{marker} {line}")))
        .sum();
    let text = if let Some(placeholder) = placeholder {
        format!("\x1b[2m{placeholder}\x1b[22m")
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
/// Codex's `›`: Bus reads the last `›` line on screen as the composer, and the
/// sent prompt must never be taken for a draft. The fake Cursor's `>` is one
/// of Cursor's composer markers; its live composer follows below, except
/// while the hooks of a finished turn run, before the idle composer is drawn.
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

/// Codex's `• Working (3s • esc to interrupt)`, Cursor's `⬢ Generating (3s)`,
/// or Claude's `✻ Working… (3s · esc to interrupt)`, at spinner `frame`.
fn working_line(provider: &str, frame: usize, seconds: u64) -> String {
    match provider {
        "codex" => {
            let bullet = ['•', '◦'][frame % 2];
            format!("{bullet} Working ({seconds}s • esc to interrupt)")
        }
        "cursor-agent" => {
            let hexagon = ['⬢', '⬡'][frame % 2];
            format!("{hexagon} Generating ({seconds}s)")
        }
        _ => {
            let spinner = ['✻', '✶', '✢', '·'][frame % 4];
            format!("{spinner} Working… ({seconds}s · esc to interrupt)")
        }
    }
}

/// A running Codex or Cursor turn: the working `line`, a blank row and the
/// live composer showing `shown` (Cursor's asks for a follow-up). Drawn first
/// from the start of the line under the transcript, then `again` in place
/// from the composer's first row, where it leaves the cursor.
fn working_block(line: &str, shown: &str, provider: &str, again: bool, cols: u16) -> String {
    let back = if again {
        work_done(line, provider, cols)
    } else {
        String::new()
    };
    format!(
        "{back}{line}\r\n\r\n{}{}",
        composer_top(provider),
        prompt_box_with(shown, provider, CURSOR_FOLLOW_UP, cols)
    )
}

/// Clears a running turn's working block, from the composer's first row, so
/// the transcript continues where its working `line` was.
fn work_done(line: &str, provider: &str, cols: u16) -> String {
    // Cursor's border wraps in a pane narrower than it, so count its rows.
    let top = match composer_top(provider) {
        top if top.is_empty() => 0,
        top => screen_rows(top.trim_end_matches("\r\n"), cols),
    };
    let above = screen_rows(line, cols) + 1 + top;
    format!("\x1b[{above}A\r\x1b[J")
}

#[derive(Debug, PartialEq, Eq)]
enum Input {
    Text(String),
    Backspace,
    /// Ctrl+K or Ctrl+U.
    KillLine,
    Enter,
    /// A lone Esc, as Bus sends the key in one write.
    Escape,
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
                    flush(&mut text, &mut events);
                    events.push(Input::Escape);
                    i += 1;
                    continue;
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
