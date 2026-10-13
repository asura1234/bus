//! Scratch-only deterministic fixtures. This is not the production policy engine.
use super::{drain_input, FakeAgent, Input};
use serde_json::{json, Value};
use std::io::Write;
use std::process::Command;
use std::time::{Duration, Instant};

pub(super) fn fixture_kind(prompt: &str) -> Option<&str> {
    let kind = prompt.strip_prefix("fake:permission ")?.trim();
    ["safe", "destructive", "question"]
        .contains(&kind)
        .then_some(kind)
}

fn screen(kind: &str) -> String {
    let title = match kind {
        "safe" => "Would you like to run the following command?\r\n\r\n$ cargo test permission_safe_probe --locked",
        "destructive" => "Would you like to run the following command?\r\n\r\n$ rm -rf /outside-worker-worktree/important-data",
        _ => "Which design should the worker use?",
    };
    let options = if kind == "question" {
        "› 1. Alpha\r\n  2. Beta"
    } else {
        "› 1. Yes, proceed (y)\r\n  2. Yes, don't ask again this session (p)\r\n  3. No, and tell Codex what to do differently (esc)"
    };
    format!("{title}\r\n\r\n{options}\r\n\r\npress enter to confirm or esc to cancel\r\n")
}

impl FakeAgent {
    /// Clear models a provider reset rather than a normal assistant turn.
    pub(super) fn fixture_control(&mut self, prompt: &str) -> Option<Option<i32>> {
        match prompt.trim() {
            "fake:exit" => Some(Some(0)),
            "/clear" => {
                self.session = pseudo_uuid();
                self.turns = 0;
                self.images = 0;
                self.draft.clear();
                self.working = None;
                self.hook("SessionStart", None, json!({"source": "clear"}));
                self.draw_prompt();
                Some(None)
            }
            _ => None,
        }
    }

    /// Hold the exact visible dialog until native input. Never execute its command.
    pub(super) fn permission_dialog(&mut self, kind: &str) -> Result<(), i32> {
        if self.live_turn() {
            self.end_turn(&[]);
        }
        let rendered = screen(kind);
        let mut out = std::io::stdout();
        let _ = write!(out, "{rendered}");
        let _ = out.flush();
        let started = Instant::now();
        let mut selected = 1i32;
        while started.elapsed() < Duration::from_secs(600) {
            let bytes = match self
                .input
                .as_ref()
                .map(|rx| rx.recv_timeout(Duration::from_millis(100)))
            {
                Some(Ok(bytes)) => bytes,
                Some(Err(std::sync::mpsc::RecvTimeoutError::Timeout)) => continue,
                _ => return Err(0),
            };
            self.pending.extend(bytes);
            for input in drain_input(&mut self.pending) {
                match input {
                    Input::Enter if kind != "safe" || selected == 1 => {
                        let rows = rendered.matches("\r\n").count();
                        let _ = write!(out, "\x1b[{rows}A\r\x1b[J");
                        let _ = out.flush();
                        return Ok(());
                    }
                    Input::Interrupt => return Err(0),
                    Input::Navigate(delta) => {
                        selected =
                            (selected + delta).clamp(1, if kind == "question" { 2 } else { 3 });
                        let rows = rendered.matches("\r\n").count();
                        let options = rendered
                            .replace("› 1.", "  1.")
                            .replace(&format!("  {selected}."), &format!("› {selected}."));
                        let _ = write!(out, "\x1b[{rows}A\r\x1b[J{options}");
                        let _ = out.flush();
                    }
                    _ => {}
                }
            }
        }
        Err(1)
    }

    /// Only the fake Claude service executes deterministic typed decisions.
    pub(super) fn permission_reply(&self, prompt: &str) -> Option<String> {
        if self.provider != "claude" || !prompt.starts_with("Permission assignment ") {
            return None;
        }
        let handler = std::env::var("BUS_PERMISSION_HANDLER_ID").ok()?;
        Some(match service_decision(&handler, prompt) {
            Ok(outcome) => format!("Permission fixture completed: {outcome}."),
            Err(error) => format!("Permission fixture failed: {error}"),
        })
    }
}

fn bus(args: &[&str]) -> Result<Value, String> {
    let binary = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|e| e.to_string())?;
    let output = Command::new(binary)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let mut trace = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("fake-permission-cli.ndjson")
        .map_err(|e| e.to_string())?;
    writeln!(trace, "{}", json!({"args":args,"success":output.status.success(),
        "stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr)}))
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}

fn service_decision(handler: &str, prompt: &str) -> Result<&'static str, String> {
    service_decision_with(handler, prompt, bus)
}

fn service_decision_with(
    handler: &str,
    prompt: &str,
    mut call: impl FnMut(&[&str]) -> Result<Value, String>,
) -> Result<&'static str, String> {
    let state = call(&["state"])?;
    let token = prompt
        .strip_prefix("Permission assignment ")
        .and_then(|s| s.split('.').next())
        .ok_or("Missing assignment token")?;
    let assignment = state["permission_assignments"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["handler"].to_string() == handler && a["token"].as_str() == Some(token))
        .ok_or("No matching active handler assignment")?;
    let worker = assignment["worker"].to_string();
    let observed = call(&["agent", "dialog", &worker])?;
    let text = observed["dialog"]["text"].as_str().unwrap_or_default();
    let (outcome, summary) = fixture_decision(text);
    let fingerprint = observed["fingerprint"].as_str().unwrap_or("unavailable");
    if outcome == "approved" {
        let answer = call(&[
            "agent",
            "choose",
            &worker,
            "--option",
            "1",
            "--fingerprint",
            fingerprint,
            "--handler",
            handler,
            "--assignment",
            token,
        ])?;
        if answer["outcome"] != "closed" {
            return Err("Native answer did not close the fixture dialog".into());
        }
    }
    call(&[
        "agent",
        "permission-decision",
        &worker,
        "--handler",
        handler,
        "--assignment",
        token,
        "--fingerprint",
        fingerprint,
        "--outcome",
        outcome,
        "--summary",
        summary,
    ])?;
    Ok(outcome)
}

fn fixture_decision(text: &str) -> (&'static str, &'static str) {
    if text.contains("$ cargo test permission_safe_probe --locked") {
        ("approved", "safe worker test command")
    } else if text.contains("$ rm -rf /outside-worker-worktree/important-data") {
        (
            "escalated",
            "destructive command outside worker scope needs orchestrator decision",
        )
    } else {
        (
            "routed",
            "not permission-related; worker choice or unknown screen needs orchestrator decision",
        )
    }
}

pub(super) fn pseudo_uuid() -> String {
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
mod tests {
    use super::*;

    #[test]
    fn permission_fixture_screens_are_native_inspectable_and_classifiable() {
        for (kind, outcome) in [
            ("safe", "approved"),
            ("destructive", "escalated"),
            ("question", "routed"),
        ] {
            let dialog =
                crate::agents::dialog::parse(&screen(kind)).expect("inspectable native screen");
            assert_eq!(fixture_decision(&dialog.text).0, outcome);
            assert_eq!(dialog.selected(), Some(1));
        }
        assert_eq!(fixture_decision("unknown screen").0, "routed");
        assert_eq!(fixture_kind("quoted fake:permission safe"), None);
    }

    #[test]
    fn permission_fixture_service_uses_observed_fingerprint_and_active_lease() {
        for (kind, expected) in [
            ("safe", "approved"),
            ("destructive", "escalated"),
            ("question", "routed"),
        ] {
            let text = crate::agents::dialog::parse(&screen(kind)).unwrap().text;
            let mut calls = Vec::new();
            let result = service_decision_with("7", "Permission assignment p8-123. Worker worker", |args| {
                calls.push(args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
                Ok(match args {
                    ["state"] => json!({"permission_assignments":[{"worker":6,"handler":7,"token":"p8-123"}]}),
                    ["agent", "dialog", "6"] => json!({"dialog":{"text":text},"fingerprint":"d1.observed"}),
                    ["agent", "choose", ..] => json!({"outcome":"closed"}),
                    ["agent", "permission-decision", ..] => json!({"recorded":true}),
                    _ => panic!("unexpected control operation: {args:?}"),
                })
            }).unwrap();
            assert_eq!(result, expected);
            let chooses: Vec<_> = calls
                .iter()
                .filter(|args| args.get(1).is_some_and(|a| a == "choose"))
                .collect();
            assert_eq!(chooses.len(), usize::from(kind == "safe"));
            if kind == "safe" {
                assert_eq!(
                    chooses[0],
                    &vec![
                        "agent",
                        "choose",
                        "6",
                        "--option",
                        "1",
                        "--fingerprint",
                        "d1.observed",
                        "--handler",
                        "7",
                        "--assignment",
                        "p8-123"
                    ]
                );
            }
            let decision = calls.last().unwrap();
            assert!(decision
                .windows(2)
                .any(|pair| pair == ["--fingerprint", "d1.observed"]));
            assert!(decision
                .windows(2)
                .any(|pair| pair == ["--outcome", expected]));
        }
    }

    #[test]
    fn permission_fixture_service_rejects_assignment_for_another_handler() {
        let mut calls = 0;
        let result =
            service_decision_with("7", "Permission assignment p8-123. Worker worker", |_| {
                calls += 1;
                Ok(json!({"permission_assignments":[{"worker":6,"handler":9,"token":"p8-123"}]}))
            });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }
}
