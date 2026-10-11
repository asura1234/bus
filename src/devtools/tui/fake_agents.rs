//! Fake provider CLIs for driver sessions. The scratch Bus finds them first on
//! its PATH, so adding a `claude`, `codex` or `cursor` agent never starts a
//! real provider or spends model usage.
//!
//! Each fake is the `bus` binary itself under the provider's name, so Bus sees
//! a process called `claude` (or `codex`, `cursor-agent`). The fake Claude
//! fires Bus's own hooks from the `--settings` file Bus passes it, draws a
//! Claude-like prompt box, and answers every prompt with `fake reply: <prompt>`.
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Set in the scratch Bus's environment; the fake only runs where it is set.
pub(super) const FAKE_AGENT_ENV: &str = "BUS_TUI_FAKE_AGENT";
pub(crate) const PROVIDER_NAMES: &[&str] = &["claude", "codex", "cursor-agent"];

/// Links each provider name in `bin_dir` to the `bus` binary.
pub(super) fn install(bin_dir: &Path, binary: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(bin_dir)?;
    for name in PROVIDER_NAMES {
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
pub(crate) fn fake_provider(argv0: &str) -> Option<&'static str> {
    std::env::var_os(FAKE_AGENT_ENV)?;
    let name = Path::new(argv0).file_stem()?.to_str()?;
    PROVIDER_NAMES
        .iter()
        .copied()
        .find(|provider| *provider == name)
}

pub(crate) fn run(provider: &str, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--version" || a == "-v") {
        let _ = writeln!(std::io::stdout(), "0.0.0 (bus tui fake {provider})");
        return 0;
    }
    let hooks = settings_path(args)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|settings| settings["hooks"].clone())
        .unwrap_or(Value::Null);
    let mut agent = FakeAgent::new(provider, hooks);
    agent.session_start();
    agent.draw_prompt("");
    if crossterm::terminal::enable_raw_mode().is_err() {
        return 1;
    }
    let code = agent.read_loop();
    let _ = crossterm::terminal::disable_raw_mode();
    code
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
}

impl FakeAgent {
    fn new(provider: &str, hooks: Value) -> Self {
        Self {
            provider: provider.to_owned(),
            hooks,
            session: pseudo_uuid(),
            turns: 0,
        }
    }

    /// Runs Bus's hook command for `event` with `payload` on stdin.
    fn hook(&self, event: &str, mut payload: Value) {
        let Some(command) = self.hooks[event][0]["hooks"][0]["command"].as_str() else {
            return;
        };
        payload["hook_event_name"] = json!(event);
        payload["session_id"] = json!(self.session);
        payload["cwd"] = json!(std::env::current_dir().ok());
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
        self.hook("SessionStart", json!({"source": "startup"}));
    }

    fn draw_prompt(&self, draft: &str) {
        let rule = "─".repeat(60);
        let mut out = std::io::stdout();
        let _ = write!(
            out,
            "\r\n{rule}\r\n❯ {draft}\r\n{rule}\r\n  ? for shortcuts · fake {}\r\n\x1b[3A\x1b[{}G",
            self.provider,
            3 + draft.chars().count()
        );
        let _ = out.flush();
    }

    fn submit(&mut self, prompt: &str) {
        self.turns += 1;
        let turn = format!("{}-{}", self.session, self.turns);
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b[3B\r\n> {prompt}\r\n");
        let _ = out.flush();
        self.hook(
            "UserPromptSubmit",
            json!({"prompt_id": turn, "prompt": prompt}),
        );
        if let Some(seconds) = work_seconds(prompt) {
            spin(seconds);
        }
        let reply = format!("fake reply: {}", prompt.trim());
        let _ = write!(out, "⏺ {reply}\r\n");
        let _ = out.flush();
        self.hook(
            "Stop",
            json!({"prompt_id": turn, "last_assistant_message": reply, "stop_hook_active": false}),
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
                        let mut out = std::io::stdout();
                        let _ = write!(out, "{}", text.replace('\n', " "));
                        let _ = out.flush();
                    }
                    Input::Backspace => {
                        if draft.pop().is_some() {
                            let mut out = std::io::stdout();
                            let _ = write!(out, "\x08 \x08");
                            let _ = out.flush();
                        }
                    }
                    Input::Enter if !draft.trim().is_empty() => {
                        let prompt = std::mem::take(&mut draft);
                        self.submit(&prompt);
                    }
                    Input::Enter => {}
                    Input::Interrupt => return 0,
                }
            }
        }
    }
}

/// A prompt containing `fake:work N` keeps the fake busy for N seconds (at
/// most 600), so a driver can see a Working agent and a live spinner.
pub(super) fn work_seconds(prompt: &str) -> Option<u64> {
    let rest = prompt.split("fake:work").nth(1)?;
    let seconds: u64 = rest.split_whitespace().next()?.parse().ok()?;
    Some(seconds.min(600))
}

/// Claude's working line, redrawn in place: `✻ Working… (3s · esc to interrupt)`.
fn spin(seconds: u64) {
    let frames = ['✻', '✶', '✢', '·'];
    let started = std::time::Instant::now();
    let mut out = std::io::stdout();
    let mut index = 0;
    while started.elapsed().as_secs() < seconds {
        let _ = write!(
            out,
            "\r\x1b[2K{} Working… ({}s · esc to interrupt)",
            frames[index % frames.len()],
            started.elapsed().as_secs()
        );
        let _ = out.flush();
        index += 1;
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    let _ = write!(out, "\r\x1b[2K");
    let _ = out.flush();
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Input {
    Text(String),
    Backspace,
    Enter,
    Interrupt,
}

/// Splits raw terminal input into text, Enter, Backspace and Ctrl+C/D,
/// unwrapping bracketed pastes and dropping other escape sequences. Bytes of
/// an unfinished sequence stay in `pending`.
pub(super) fn drain_input(pending: &mut Vec<u8>) -> Vec<Input> {
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
            b'\r' | b'\n' => {
                flush(&mut text, &mut events);
                events.push(Input::Enter);
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
