//! Synthetic Grok Build fixture. Never invokes Grok or executes prompt commands.
//! Session envelopes mirror offline 1.0.5 captures; turn/card schemas are
//! provisional synthetic contracts, pending authenticated native evidence.
use super::input::{drain_input, Input};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) fn run(args: &[String]) -> i32 {
    run_with_output(args, &mut io::stdout())
}

fn run_with_output(args: &[String], output: &mut impl Write) -> i32 {
    if args.iter().any(|arg| arg == "--version" || arg == "-v") {
        let _ = writeln!(output, "grok 1.0.5 (bus-synthetic-fake)");
        return 0;
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        let _ = writeln!(output, "Grok Build TUI\nSYNTHETIC Bus fixture\nUsage: grok [OPTIONS]\n--no-leader --fullscreen --no-auto-update --resume UUID --session-id UUID --permission-mode MODE --prompt-json JSON");
        return 0;
    }
    if std::env::var_os(super::FAKE_AGENT_ENV).is_none() {
        eprintln!("Synthetic Grok requires the isolated Bus TUI driver");
        return 78;
    }
    match runtime(args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Synthetic Grok: {error}");
            1
        }
    }
}

fn runtime(args: &[String]) -> io::Result<()> {
    let adopted = resume_session(args)?;
    let cwd = std::env::current_dir()?;
    let hooks = discover_hooks(&cwd)?;
    let mut g = Grok::new(
        cwd,
        adopted.clone().unwrap_or_else(super::pseudo_uuid),
        hooks,
    )?;
    g.emit(
        "SessionStart",
        None,
        json!({"source":if adopted.is_some() {"resume"} else {"new"}}),
    )?;
    crossterm::terminal::enable_raw_mode()?;
    let result = g.read_loop();
    let _ = crossterm::terminal::disable_raw_mode();
    // Session-level shutdown Stop must never pretend to complete a prompt.
    g.emit("Stop", None, json!({"reason":"shutdown"}))?;
    result
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn resume_session(args: &[String]) -> io::Result<Option<String>> {
    let mut found = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        let value = if arg == "--resume" {
            Some(
                args.next()
                    .ok_or_else(|| invalid("missing --resume session"))?
                    .clone(),
            )
        } else {
            arg.strip_prefix("--resume=").map(str::to_owned)
        };
        if let Some(value) = value {
            if found.is_some() || value.is_empty() || value.starts_with('-') {
                return Err(invalid("invalid or repeated --resume"));
            }
            found = Some(value);
        }
    }
    Ok(found)
}

/// Only project hooks, never user/global settings. Stop at the nearest source
/// or repository root, so a scratch fixture cannot reach a user's ~/.grok.
fn discover_hooks(cwd: &Path) -> io::Result<Value> {
    for dir in cwd.ancestors() {
        let source = dir.join(".grok/hooks");
        if source.is_dir() {
            let mut files = std::fs::read_dir(source)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()?;
            files.sort();
            let mut merged = serde_json::Map::new();
            for path in files
                .into_iter()
                .filter(|path| path.extension().is_some_and(|e| e == "json"))
            {
                let value: Value = serde_json::from_str(&std::fs::read_to_string(path)?)
                    .map_err(|error| invalid(&error.to_string()))?;
                let hooks = value["hooks"]
                    .as_object()
                    .ok_or_else(|| invalid("invalid project hook object"))?;
                for (event, entries) in hooks {
                    let entries = entries
                        .as_array()
                        .ok_or_else(|| invalid("hook entries must be arrays"))?;
                    merged
                        .entry(event.clone())
                        .or_insert_with(|| json!([]))
                        .as_array_mut()
                        .ok_or_else(|| invalid("synthetic hook entries must be arrays"))?
                        .extend(entries.iter().cloned());
                }
            }
            return Ok(Value::Object(merged));
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    Ok(json!({}))
}

fn clip(text: &str) -> String {
    text.chars().take(32768).collect()
}

struct Active {
    id: String,
    prompts: Vec<String>,
    due: Instant,
    source: String,
}

struct Card {
    selected: usize,
    project_scope: bool,
    focused: bool,
    free_text: bool,
    question: bool,
    answer: String,
}

struct Grok {
    cwd: PathBuf,
    session: String,
    hooks: Value,
    transcript: PathBuf,
    turns: u64,
    sequence: u64,
    active: Option<Active>,
    card: Option<Card>,
    wake: Option<Instant>,
    draft: String,
    pending: Vec<u8>,
    card_pending: Vec<u8>,
    last: String,
}

impl Grok {
    fn new(cwd: PathBuf, session: String, hooks: Value) -> io::Result<Self> {
        if session.is_empty()
            || session.len() > 128
            || !session
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(invalid("unsafe synthetic session name"));
        }
        let root = cwd.join(".grok/bus-fake").join(&session);
        std::fs::create_dir_all(&root)?;
        let transcript = root.join("updates.jsonl");
        let previous = match std::fs::read_to_string(&transcript) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let mut turns = 0;
        let mut sequence = 0;
        for line in previous.lines() {
            let record: Value = serde_json::from_str(line)
                .map_err(|_| invalid("cannot resume partial synthetic transcript"))?;
            sequence += 1;
            if let Some(id) = record["params"]["update"]["promptId"].as_str() {
                if let Some(number) = id.rsplit('-').next().and_then(|n| n.parse::<u64>().ok()) {
                    turns = turns.max(number);
                }
            }
        }
        std::fs::write(
            root.join("summary.json"),
            json!({"sessionId":session,"synthetic":true}).to_string(),
        )?;
        Ok(Self {
            cwd,
            session,
            hooks,
            transcript,
            turns,
            sequence,
            active: None,
            card: None,
            wake: None,
            draft: String::new(),
            pending: Vec::new(),
            card_pending: Vec::new(),
            last: String::new(),
        })
    }

    fn emit(&self, event: &str, turn: Option<&str>, mut payload: Value) -> io::Result<()> {
        let native = match event {
            "SessionStart" => "session_start",
            "UserPromptSubmit" => "user_prompt_submit",
            "StopFailure" => "stop_failure",
            "StopCancelled" => "stop_cancelled",
            "Notification" => "notification",
            _ => "stop",
        };
        payload["hookEventName"] = json!(native);
        payload["sessionId"] = json!(self.session);
        payload["cwd"] = json!(self.cwd);
        payload["transcriptPath"] = json!(self.transcript);
        payload["synthetic"] = json!(true);
        if event == "Stop" && turn.is_some() && payload.get("reason").is_none() {
            payload["reason"] = json!("end_turn");
        }
        if let Some(turn) = turn {
            payload["promptId"] = json!(turn);
        }
        for entry in self.hooks[event].as_array().into_iter().flatten() {
            for hook in entry["hooks"].as_array().into_iter().flatten() {
                if hook["type"] != "command" {
                    continue;
                }
                if let Some(command) = hook["command"].as_str() {
                    let mut child =
                        std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
                            .args([if cfg!(windows) { "/C" } else { "-c" }, command])
                            .current_dir(&self.cwd)
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn()?;
                    if let Some(mut stdin) = child.stdin.take() {
                        stdin.write_all(payload.to_string().as_bytes())?;
                    }
                    if !child.wait()?.success() {
                        return Err(invalid("synthetic project hook failed"));
                    }
                }
            }
        }
        Ok(())
    }

    fn update(&mut self, mut update: Value, turn: &str) -> io::Result<()> {
        self.sequence += 1;
        update["promptId"] = json!(turn);
        let record = json!({
            "method":"_x.ai/session/update",
            "params":{"sessionId":self.session, "update":update,
                "_meta":{"eventId":format!("{}-{}", self.session, self.sequence),
                         "promptId":turn, "synthetic":true}}
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.transcript)?;
        writeln!(file, "{record}")
    }

    fn message(&mut self, turn: &str, role: &str, text: &str) -> io::Result<()> {
        self.update(
            json!({"sessionUpdate":format!("{role}_message_chunk"),
            "content":{"type":"text","text":text}}),
            turn,
        )?;
        let mut raw = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.transcript.with_file_name("chat_history.jsonl"))?;
        writeln!(
            raw,
            "{}",
            json!({"sessionId":self.session,"promptId":turn,
            "role":role,"content":text,"synthetic":true})
        )
    }

    fn submit(&mut self, prompt: &str, now: Instant) -> io::Result<()> {
        self.submit_source(prompt, now, "keyboard")
    }

    fn submit_source(&mut self, prompt: &str, now: Instant, source: &str) -> io::Result<()> {
        if let Some(active) = self.active.as_mut() {
            active.prompts.push(prompt.to_owned());
            let id = active.id.clone();
            self.emit(
                "UserPromptSubmit",
                Some(&id),
                json!({"prompt":prompt,"source":source}),
            )?;
            return self.message(&id, "user", prompt);
        }
        self.turns += 1;
        let id = format!("{}-{}", self.session, self.turns);
        let delay = super::directives::work_seconds(prompt)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_millis(150));
        self.active = Some(Active {
            id: id.clone(),
            prompts: vec![prompt.into()],
            due: now + delay,
            source: source.into(),
        });
        self.update(json!({"sessionUpdate":"turn_start","source":source}), &id)?;
        self.emit(
            "UserPromptSubmit",
            Some(&id),
            json!({"prompt":prompt,"source":source}),
        )?;
        self.message(&id, "user", prompt)?;
        if prompt.contains("fake:gate") {
            self.emit(
                "Stop",
                Some(&id),
                json!({"lastAssistantMessage":"synthetic gate attempt","stopHookActive":true}),
            )?;
        }
        if ["fake:card", "fake:question", "fake:free-text"]
            .iter()
            .any(|s| prompt.contains(s))
        {
            self.card = Some(Card {
                selected: if prompt.contains("fake:free-text") {
                    2
                } else {
                    0
                },
                project_scope: false,
                focused: true,
                free_text: prompt.contains("fake:free-text"),
                question: prompt.contains("fake:question") || prompt.contains("fake:free-text"),
                answer: String::new(),
            });
        }
        Ok(())
    }

    fn cancel(&mut self) -> io::Result<()> {
        self.card = None;
        if let Some(active) = self.active.take() {
            self.update(
                json!({"sessionUpdate":"turn_complete","stopReason":"cancelled"}),
                &active.id,
            )?;
            self.emit(
                "StopCancelled",
                Some(&active.id),
                json!({"reason":"cancelled"}),
            )?;
            self.last = "Synthetic turn cancelled".into();
        }
        Ok(())
    }

    fn tick(&mut self, now: Instant) -> io::Result<()> {
        if self.active.is_none() && self.wake.is_some_and(|at| now >= at) {
            self.wake = None;
            self.emit(
                "Notification",
                None,
                json!({"notificationType":"background_task","message":"synthetic task completed"}),
            )?;
            self.submit_source("synthetic background completion", now, "background_task")?;
        }
        if self.card.is_some() || self.active.as_ref().is_none_or(|active| now < active.due) {
            return Ok(());
        }
        let Some(active) = self.active.take() else {
            return Ok(());
        };
        let prompt = active.prompts.join("\n");
        if prompt.contains("fake:cancel") || prompt.contains("fake:gate-cancel") {
            self.active = Some(active);
            return self.cancel();
        }
        if prompt.contains("fake:api-error") {
            self.update(
                json!({"sessionUpdate":"turn_complete","stopReason":"error"}),
                &active.id,
            )?;
            self.last = "Synthetic API Error: Connection lost".into();
            self.emit(
                "StopFailure",
                Some(&active.id),
                json!({"error":"server_error","lastAssistantMessage":self.last}),
            )?;
            return Ok(());
        }
        let reply = if prompt.contains("fake:long") {
            "界".repeat(40000)
        } else {
            super::directives::reply(&prompt)
        };
        self.message(&active.id, "agent", &reply)?;
        if prompt.contains("fake:partial") {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.transcript)?;
            write!(file, "{{\"method\":\"_x.ai/session/update\",\"params\":")?;
        } else {
            self.update(
                json!({"sessionUpdate":"turn_complete","stopReason":"end_turn",
                "source":active.source}),
                &active.id,
            )?;
        }
        if prompt.contains("fake:no-final-stop") {
            self.emit(
                "Notification",
                None,
                json!({"notificationType":"idle_prompt"}),
            )?;
        } else {
            self.emit(
                "Stop",
                Some(&active.id),
                json!({"lastAssistantMessage":clip(&reply)}),
            )?;
        }
        self.last = reply.chars().take(240).collect();
        if let Some(seconds) = prompt
            .split_once("fake:wake ")
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
        {
            self.wake = Some(now + Duration::from_secs(seconds.min(600)));
        }
        Ok(())
    }

    /// Inert card controls: no displayed tool command executes.
    fn card_key(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.card_pending.extend_from_slice(bytes);
        while self.card.is_some() && !self.card_pending.is_empty() {
            let rest = &self.card_pending;
            let length = if rest.starts_with(b"\x1b[200~") {
                let Some(at) = rest.windows(6).position(|w| w == b"\x1b[201~") else {
                    break;
                };
                at + 6
            } else if b"\x1b[200~".starts_with(rest) && rest.len() > 1 {
                break;
            } else if rest.starts_with(b"\x1b[") || rest.starts_with(b"\x1bO") {
                let Some(at) = rest[2..].iter().position(|b| (0x40..=0x7e).contains(b)) else {
                    break;
                };
                at + 3
            } else {
                let length = match rest[0] {
                    0x00..=0x7f => 1,
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => return Err(invalid("invalid synthetic card UTF-8")),
                };
                if rest.len() < length {
                    break;
                }
                length
            };
            let key: Vec<u8> = self.card_pending.drain(..length).collect();
            self.card_key_one(&key)?;
        }
        if self.card.is_none() {
            self.card_pending.clear();
        }
        Ok(())
    }

    fn card_key_one(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes == b"\x03" {
            return self.cancel();
        }
        let Some(card) = self.card.as_mut() else {
            return Ok(());
        };
        match bytes {
            b"\x1b" if card.free_text => card.free_text = false,
            b"\x1b" => card.focused = false,
            b"X" if card.question && !card.free_text => {
                self.card = None;
            }
            b"\x1b[A" | b"\x1bOA" => card.selected = card.selected.saturating_sub(1),
            b"\x1b[B" | b"\x1bOB" => card.selected = (card.selected + 1).min(2),
            b"\x1b[C" | b"\x1b[D" => card.project_scope = !card.project_scope,
            b"\t" => card.focused = !card.focused,
            b"z" if card.question && !card.free_text => {
                card.free_text = true;
                card.selected = 2;
            }
            b"1" if !card.free_text => {
                card.selected = 0;
                card.focused = true;
                if card.question {
                    return self.card_key_one(b"\r");
                }
            }
            b"2" if !card.free_text => {
                card.selected = 1;
                card.focused = true;
                if card.question {
                    return self.card_key_one(b"\r");
                }
            }
            b"\r" | b"\x1b[13u" if card.focused => {
                if card.question && card.selected == 2 && !card.free_text {
                    card.free_text = true;
                    return Ok(());
                }
                if card.selected == 1 && !card.question {
                    return self.cancel();
                }
                if card.free_text && card.answer.trim().is_empty() {
                    return Ok(());
                }
                let answer = card.answer.clone();
                self.card = None;
                if let Some(active) = self.active.as_mut() {
                    if !answer.is_empty() {
                        active.prompts.push(answer);
                    }
                }
            }
            _ if card.free_text && card.focused => {
                for input in drain_input(&mut bytes.to_vec()) {
                    match input {
                        Input::Text(text) => card.answer.push_str(&text),
                        Input::Backspace => {
                            card.answer.pop();
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn screen(&self) -> String {
        let (title, progress) = if self.card.is_some() {
            ("⚠ Action Required - grok", "4;0;0")
        } else if self.active.is_some() {
            ("⠋ Working - grok", "4;1;-1")
        } else {
            ("grok", "4;0;0")
        };
        let mut screen = format!(
            "\x1b]0;{title}\x07\x1b]9;{progress}\x07SYNTHETIC Grok Build 1.0.5 · {}\r\n{}\r\n",
            self.session, self.last
        );
        if let Some(card) = &self.card {
            screen.push_str(if card.question {
                "◆ Choose an answer\r\n┃ Question 1/1\r\n"
            } else {
                "◆ Allow synthetic shell command?\r\n"
            });
            if !card.question {
                screen.push_str(if card.project_scope {
                    "┃ Scope: all commands\r\n"
                } else {
                    "┃ Scope: this command\r\n"
                });
            }
            let labels = ["Yes, proceed", "No, reject", "Type your answer here"];
            for (i, label) in labels
                .iter()
                .enumerate()
                .take(if card.question { 3 } else { 2 })
            {
                let selector = if i == 2 {
                    "z".to_owned()
                } else {
                    (i + 1).to_string()
                };
                let label: &str = if i == 2 && !card.answer.is_empty() {
                    card.answer.as_str()
                } else {
                    label
                };
                screen.push_str(&format!(
                    "┃ {}{} (○) {label}\r\n",
                    if card.focused && i == card.selected {
                        "❯ "
                    } else {
                        "  "
                    },
                    selector
                ));
            }
            screen.push_str(if !card.focused {
                if card.question {
                    "Tab/Space: question\r\n"
                } else {
                    "Tab/Space: permission\r\n"
                }
            } else if card.free_text {
                "Enter:submit │ Esc:back\r\n"
            } else if card.question {
                "1/2:select │ Shift+x:dismiss\r\n"
            } else {
                "1/2:select │ ←/→:scope │ Ctrl+o:yolo │ Ctrl+c:cancel\r\n"
            });
            return screen;
        } else if self.active.is_some() {
            screen.push_str("⠋ Working… [stop]\r\n");
        }
        screen.push_str(&format!(
            "╭{}╮\r\n│ ❯ {} │\r\n╰{}╯\r\n{}Shift+Tab:mode │ Ctrl+x:shortcuts\r\n",
            "─".repeat(58),
            self.draft.replace('\n', " │\r\n│   "),
            "─".repeat(58),
            if self.active.is_some() {
                "Esc:cancel │ "
            } else {
                ""
            }
        ));
        screen
    }

    fn draw(&self) -> io::Result<()> {
        let mut out = io::stdout();
        write!(out, "\x1b[H\x1b[2J{}", self.screen())?;
        out.flush()
    }

    fn read_loop(&mut self) -> io::Result<()> {
        let receiver = super::read_input();
        self.draw()?;
        loop {
            let now = Instant::now();
            self.tick(now)?;
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(bytes) if self.card.is_some() => self.card_key(&bytes)?,
                Ok(bytes) => {
                    self.pending.extend(bytes);
                    for input in drain_input(&mut self.pending) {
                        match input {
                            Input::Text(text) => self.draft.push_str(&text),
                            Input::Backspace => {
                                self.draft.pop();
                            }
                            Input::KillLine => super::kill_line(&mut self.draft),
                            Input::Enter if !self.draft.trim().is_empty() => {
                                let prompt = std::mem::take(&mut self.draft);
                                self.submit(&prompt, now)?;
                            }
                            Input::Escape if self.active.is_some() => self.cancel()?,
                            Input::Interrupt if self.active.is_some() => self.cancel()?,
                            Input::Interrupt => return Ok(()),
                            _ => {}
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.draw()?;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn grok_f_version_identifies_build_without_model_work() {
        let mut output = Vec::new();
        assert_eq!(
            super::run_with_output(&["--version".into()], &mut output),
            0
        );
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "grok 1.0.5 (bus-synthetic-fake)\n"
        );
    }

    #[test]
    fn grok_f_help_is_available_without_starting_runtime() {
        let mut output = Vec::new();
        assert_eq!(super::run_with_output(&["--help".into()], &mut output), 0);
        let text = String::from_utf8(output).unwrap();
        assert!(text.lines().any(|line| line == "Grok Build TUI"));
        for required in [
            "Usage: grok",
            "--resume",
            "--session-id",
            "--fullscreen",
            "--permission-mode",
            "--prompt-json",
        ] {
            assert!(text.contains(required), "{required} missing from {text}");
        }
    }
}

#[cfg(test)]
mod scenario_tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::Path;
    use std::time::{Duration, Instant};

    #[test]
    fn grok_g_fake_cards_match_joined_dialog_contract() {
        let root = Scratch::new();
        for prompt in ["fake:card", "fake:question", "fake:free-text"] {
            let mut g = agent(root.path());
            g.submit(prompt, Instant::now()).unwrap();
            let screen = g.screen();
            let dialog = crate::agents::dialog::parse(&screen)
                .expect("fake must use the supported synthetic contract");
            assert!(!dialog.options.is_empty() || dialog.input.is_some());
        }
    }

    #[test]
    fn grok_g_question_shortcut_selects_and_finishes_the_card() {
        let root = Scratch::new();
        let mut g = agent(root.path());
        g.submit("fake:question", Instant::now()).unwrap();
        g.card_key(b"1").unwrap();
        assert!(
            g.card.is_none(),
            "Question shortcuts select directly without Enter"
        );
    }

    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("bus-grok-f-{}", super::super::pseudo_uuid()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn agent(root: &Path) -> Grok {
        Grok::new(
            root.to_owned(),
            "11111111-1111-4111-8111-111111111111".into(),
            json!({}),
        )
        .unwrap()
    }

    fn updates(agent: &Grok) -> Vec<Value> {
        std::fs::read_to_string(&agent.transcript)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    #[test]
    fn grok_f_discovers_only_nearest_project_hooks_and_preserves_order() {
        let root = Scratch::new();
        let hooks = root.path().join(".grok/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(
            hooks.join("a.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"first"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(
            hooks.join("bus.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"second"}]}]}}"#,
        )
        .unwrap();
        let child = root.path().join("nested");
        std::fs::create_dir(&child).unwrap();
        let found = discover_hooks(&child).unwrap();
        assert_eq!(found["Stop"][0]["hooks"][0]["command"], "first");
        assert_eq!(found["Stop"][1]["hooks"][0]["command"], "second");
        std::fs::write(hooks.join("bad.json"), "{").unwrap();
        assert!(discover_hooks(&child).is_err());
    }

    #[test]
    fn grok_f_camelcase_hooks_nested_commands_and_shutdown_identity() {
        let root = Scratch::new();
        let output = root.path().join("hooks.jsonl");
        let hook = json!([{"hooks":[{"type":"command", "command":format!("cat >> '{}'; printf '\\n' >> '{}'", output.display(), output.display())}]}]);
        let mut g = Grok::new(
            root.path().into(),
            "session".into(),
            json!({"SessionStart":hook.clone(), "UserPromptSubmit":hook.clone(), "Stop":hook}),
        )
        .unwrap();
        g.emit("SessionStart", None, json!({"source":"new"}))
            .unwrap();
        g.submit("hello", Instant::now()).unwrap();
        g.tick(Instant::now() + Duration::from_secs(1)).unwrap();
        g.emit("Stop", None, json!({"reason":"shutdown"})).unwrap();
        let payloads: Vec<Value> = std::fs::read_to_string(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(payloads.len(), 4);
        assert_eq!(payloads[0]["hookEventName"], "session_start");
        assert_eq!(payloads[0]["sessionId"], "session");
        assert_eq!(payloads[1]["hookEventName"], "user_prompt_submit");
        assert_eq!(payloads[1]["promptId"], payloads[2]["promptId"]);
        assert!(payloads[2]["transcriptPath"]
            .as_str()
            .unwrap()
            .ends_with("updates.jsonl"));
        assert!(payloads[3].get("promptId").is_none());
    }

    #[test]
    fn grok_f_full_unicode_answer_is_preserved_while_hook_clips() {
        let root = Scratch::new();
        let mut g = agent(root.path());
        let output = root.path().join("stop.jsonl");
        g.hooks = json!({"Stop":[{"hooks":[{"type":"command", "command":format!("cat >> '{}'", output.display())}]}]});
        let now = Instant::now();
        g.submit("fake:long", now).unwrap();
        g.tick(now + Duration::from_secs(1)).unwrap();
        let records = updates(&g);
        let text = records
            .iter()
            .find_map(|r| {
                (r["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
                    .then(|| r["params"]["update"]["content"]["text"].as_str())
                    .flatten()
            })
            .unwrap();
        assert_eq!(text, "界".repeat(40000));
        assert!(records
            .iter()
            .all(|r| r["params"]["_meta"]["synthetic"] == true));
        assert!(records
            .iter()
            .any(|r| r["params"]["update"]["sessionUpdate"] == "turn_complete"));
        let hook: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(hook["hookEventName"], "stop");
        assert_eq!(hook["sessionId"], g.session);
        assert_eq!(hook["lastAssistantMessage"], "界".repeat(32768));
    }

    #[test]
    fn grok_f_gate_cancel_and_errors_never_record_a_successful_final() {
        for prompt in ["fake:gate-cancel", "fake:api-error", "fake:cancel"] {
            let root = Scratch::new();
            let mut g = agent(root.path());
            let now = Instant::now();
            g.submit(prompt, now).unwrap();
            g.tick(now + Duration::from_secs(2)).unwrap();
            assert!(!updates(&g)
                .iter()
                .any(
                    |r| r["params"]["update"]["sessionUpdate"] == "turn_complete"
                        && r["params"]["update"]["stopReason"] == "end_turn"
                ));
        }
    }

    #[test]
    fn grok_f_partial_transcript_suffix_stays_uncommitted() {
        let root = Scratch::new();
        let mut g = agent(root.path());
        g.submit("fake:partial", Instant::now()).unwrap();
        g.tick(Instant::now() + Duration::from_secs(1)).unwrap();
        let text = std::fs::read_to_string(&g.transcript).unwrap();
        assert!(!text.ends_with('\n'));
        assert!(serde_json::from_str::<Value>(text.lines().last().unwrap()).is_err());
        assert!(!updates(&g)
            .iter()
            .any(|r| r["params"]["update"]["sessionUpdate"] == "turn_complete"));
    }

    #[test]
    fn grok_f_timer_wake_owns_a_new_turn_without_keyboard_submission() {
        let root = Scratch::new();
        let mut g = agent(root.path());
        let now = Instant::now();
        g.submit("fake:wake 2", now).unwrap();
        g.tick(now + Duration::from_secs(1)).unwrap();
        let first_turns = g.turns;
        g.tick(now + Duration::from_secs(2)).unwrap();
        assert_eq!(g.turns, first_turns);
        g.tick(now + Duration::from_secs(4)).unwrap();
        g.tick(now + Duration::from_secs(5)).unwrap();
        assert_eq!(g.turns, first_turns + 1);
        let records = updates(&g);
        assert!(records
            .iter()
            .any(|r| r["params"]["update"]["source"] == "background_task"));
    }

    #[test]
    fn grok_f_card_scope_and_parked_focus_do_not_approve() {
        let root = Scratch::new();
        let mut g = agent(root.path());
        let now = Instant::now();
        g.submit("fake:card", now).unwrap();
        let original = g.screen();
        g.card_key(b"\x1b[C").unwrap();
        assert_ne!(g.screen(), original);
        g.card_key(b"\t").unwrap();
        g.card_key(b"\r").unwrap();
        g.tick(now + Duration::from_secs(1)).unwrap();
        assert!(g.card.is_some());
        g.card_key(b"\t").unwrap();
        g.card_key(b"\r").unwrap();
        g.tick(now + Duration::from_secs(2)).unwrap();
        assert!(g.card.is_none());
    }

    #[test]
    fn grok_f_live_steering_shares_the_active_turn_and_escape_cancels() {
        let root = Scratch::new();
        let mut g = agent(root.path());
        let now = Instant::now();
        g.submit("fake:work 5 first", now).unwrap();
        let turn = g.turns;
        g.submit("second", now).unwrap();
        assert_eq!(g.turns, turn);
        g.cancel().unwrap();
        assert!(g.active.is_none());
        assert!(updates(&g)
            .iter()
            .any(|r| r["params"]["update"]["stopReason"] == "cancelled"));
    }

    #[test]
    fn grok_f_resume_selector_preserves_uuid_and_rejects_missing_value() {
        assert_eq!(
            resume_session(&["--resume=uuid".into()]).unwrap(),
            Some("uuid".into())
        );
        assert_eq!(
            resume_session(&["--resume".into(), "uuid".into()]).unwrap(),
            Some("uuid".into())
        );
        assert!(resume_session(&["--resume".into()]).is_err());
        assert_eq!(
            resume_session(&["--".into(), "--resume".into()]).unwrap(),
            None
        );
    }
}

#[cfg(test)]
mod input_regression_tests {
    use super::*;

    #[test]
    fn grok_f_card_stream_handles_fragmented_arrows_and_coalesced_answer() {
        let root =
            std::env::temp_dir().join(format!("grok-f-stream-{}", super::super::pseudo_uuid()));
        std::fs::create_dir(&root).unwrap();
        let mut g = Grok::new(root.clone(), "stream".into(), json!({})).unwrap();
        g.submit("fake:card", Instant::now()).unwrap();
        let before = g.screen();
        g.card_key(b"\x1b[").unwrap();
        g.card_key(b"C").unwrap();
        assert_ne!(g.screen(), before, "scope key split between reads was lost");
        // Free-text belongs to the question card, not a permission approval.
        let mut g = Grok::new(root.clone(), "stream-question".into(), json!({})).unwrap();
        g.submit("fake:question", Instant::now()).unwrap();
        g.card_key(b"zhello\r").unwrap();
        assert!(g.card.is_none(), "coalesced free text and Enter were lost");
        assert!(g
            .active
            .as_ref()
            .unwrap()
            .prompts
            .iter()
            .any(|p| p == "hello"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn grok_f_resume_never_reuses_a_synthetic_turn_id_or_escapes_project() {
        let root =
            std::env::temp_dir().join(format!("grok-f-resume-{}", super::super::pseudo_uuid()));
        std::fs::create_dir(&root).unwrap();
        assert!(Grok::new(root.clone(), "../escape".into(), json!({})).is_err());
        let mut first = Grok::new(root.clone(), "resume".into(), json!({})).unwrap();
        let now = Instant::now();
        first.submit("first", now).unwrap();
        first.tick(now + Duration::from_secs(1)).unwrap();
        let mut resumed = Grok::new(root.clone(), "resume".into(), json!({})).unwrap();
        resumed.submit("second", now).unwrap();
        assert_eq!(resumed.turns, 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
