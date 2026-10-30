//! A scriptable Bus-native stand-in used only by private dev sessions.
mod script;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use script::{Action, Scenario};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

pub(super) fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(code) => code,
        Err(error) => {
            let _ = writeln!(io::stderr(), "fake agent: {error}");
            1
        }
    }
}

fn run_inner(args: &[String]) -> Result<i32, String> {
    let index = args
        .iter()
        .position(|arg| arg == "--script")
        .ok_or("Missing --script")?;
    let script = args.get(index + 1).ok_or("Missing --script path")?;
    let spool =
        PathBuf::from(std::env::var_os("BUS_CALLBACK_DIR").ok_or("Missing callback directory")?);
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(spool.join("manifest.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Invalid callback manifest: {e}"))?;
    let agent = manifest["agent_id"]
        .as_u64()
        .ok_or("Missing agent identity")?
        .to_string();
    let mut scenario = Scenario::load(&PathBuf::from(script), spool.join("fake-state.json"))?;
    let session = args
        .iter()
        .position(|arg| arg == "--resume")
        .and_then(|index| args.get(index + 1))
        .cloned()
        .unwrap_or_else(|| {
            format!(
                "fake-{}-{}",
                std::process::id(),
                crate::utils::time::now_ns()
            )
        });
    callback(json!({"event":"session", "session":session}))?;
    crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    let _raw = Raw;
    crossterm::execute!(io::stdout(), event::EnableBracketedPaste).map_err(|e| e.to_string())?;
    let mut turn = 0u64;
    loop {
        let Some(prompt) = read_prompt()? else {
            return Ok(0);
        };
        turn += 1;
        let turn_id = format!("{session}-{turn}");
        callback(json!({"event":"started", "session":session, "turn":turn_id, "prompt":prompt}))?;
        write!(io::stdout(), "\r\n✻ Working… (0s · esc to interrupt)\r\n")
            .map_err(|e| e.to_string())?;
        io::stdout().flush().map_err(|e| e.to_string())?;
        let mut choice = String::new();
        for action in scenario.actions(&prompt)? {
            match action {
                Action::Sleep { ms } => std::thread::sleep(Duration::from_millis(ms)),
                Action::Dialog { title, options } => choice = dialog(&title, &options)?,
                Action::Send { room, to, text } => send(&agent, &room, &to, &text)?,
                Action::Reply { text } => {
                    let text = text.replace("{{choice}}", &choice);
                    callback(
                        json!({"event":"final", "session":session, "turn":turn_id, "text":text}),
                    )?;
                    write!(io::stdout(), "\r\n{text}\r\n").map_err(|e| e.to_string())?;
                }
                Action::Fail => {
                    callback(json!({"event":"failure", "session":session, "turn":turn_id}))?
                }
                Action::Crash { code } => return Ok(code),
            }
        }
    }
}

struct Raw;
impl Drop for Raw {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), event::DisableBracketedPaste);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn callback(payload: Value) -> Result<(), String> {
    let binary = std::fs::canonicalize(std::env::current_exe().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut child = Command::new(binary)
        .args(["--bus-callback", "fake-hook"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("Callback stdin unavailable")?
        .write_all(payload.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    if !child.wait().map_err(|e| e.to_string())?.success() {
        return Err("Fake callback failed".into());
    }
    Ok(())
}

fn send(agent: &str, room: &str, to: &str, text: &str) -> Result<(), String> {
    let binary = std::fs::canonicalize(std::env::current_exe().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let output = Command::new(binary)
        .args([
            "send", "--room", room, "--as", agent, "--to", to, "--text", text,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    let response: Value =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("Invalid send receipt: {e}"))?;
    if !output.status.success() || response["ok"] != true {
        return Err(format!("Fake send failed: {response}"));
    }
    Ok(())
}

fn read_prompt() -> Result<Option<String>, String> {
    let mut draft = String::new();
    write!(io::stdout(), "\r\n").map_err(|e| e.to_string())?;
    loop {
        draw_prompt(&draft)?;
        match event::read().map_err(|e| e.to_string())? {
            Event::Paste(text) => draft.push_str(&text),
            Event::Key(key) => match key.code {
                KeyCode::Enter if !draft.trim().is_empty() => {
                    write!(io::stdout(), "{}", super::fake_agents::submitted(&draft))
                        .map_err(|e| e.to_string())?;
                    return Ok(Some(draft));
                }
                KeyCode::Backspace => {
                    draft.pop();
                }
                KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None)
                }
                KeyCode::Char('u' | 'k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    draft.clear()
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => draft.push(c),
                _ => {}
            },
            _ => {}
        }
    }
}

fn draw_prompt(draft: &str) -> Result<(), String> {
    let columns = crossterm::terminal::size().map_or(80, |(columns, _)| columns);
    let rows: usize = draft
        .split('\n')
        .map(|line| super::fake_agents::screen_rows(&format!("fake> {line}"), columns))
        .sum();
    write!(
        io::stdout(),
        "\r\x1b[Jfake> {}\r\n  scriptable agent\r\n\x1b[{}A\r\x1b[{}C",
        draft.replace('\n', "\r\n      "),
        rows + 1,
        6 + draft.rsplit('\n').next().unwrap_or("").chars().count()
    )
    .map_err(|e| e.to_string())?;
    io::stdout().flush().map_err(|e| e.to_string())
}

fn dialog(title: &str, options: &[String]) -> Result<String, String> {
    let mut selected = 0usize;
    write!(io::stdout(), "\x1b[?1049h").map_err(|e| e.to_string())?;
    loop {
        write!(io::stdout(), "\x1b[H\x1b[J{title}\r\n").map_err(|e| e.to_string())?;
        for (i, option) in options.iter().enumerate() {
            write!(
                io::stdout(),
                "{} {}. {option}\r\n",
                if i == selected { "❯" } else { " " },
                i + 1
            )
            .map_err(|e| e.to_string())?;
        }
        write!(io::stdout(), "enter to select · esc to cancel\r\n").map_err(|e| e.to_string())?;
        io::stdout().flush().map_err(|e| e.to_string())?;
        if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => selected = (selected + 1).min(options.len() - 1),
                KeyCode::Enter | KeyCode::Esc => {
                    let value = if key.code == KeyCode::Enter {
                        options[selected].clone()
                    } else {
                        String::new()
                    };
                    write!(io::stdout(), "\x1b[?1049l").map_err(|e| e.to_string())?;
                    return Ok(value);
                }
                _ => {}
            }
        }
    }
}
