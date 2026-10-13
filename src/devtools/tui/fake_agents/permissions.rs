//! Scratch-only deterministic fixtures. This is not the production policy engine.
use super::{drain_input, FakeAgent, Input};
use serde_json::{json, Value};
use std::io::Write;
use std::process::Command;
use std::time::{Duration, Instant};

pub(super) fn service_prompt(provider: &str, prompt: &str) -> bool {
    provider == "claude"
        && prompt.starts_with("Permission assignment ")
        && std::env::var_os("BUS_PERMISSION_HANDLER_ID").is_some()
}

/// A real Claude turn announces activity before hooks/tool protocol can block.
pub(super) fn begin_work(out: &mut impl Write, active: bool) {
    if active {
        let _ = write!(out, "{}\r\n", super::working_line("claude", 0, 0));
        let _ = out.flush();
    }
}

pub(super) fn end_work(out: &mut impl Write, active: bool) {
    if active {
        let cols = crossterm::terminal::size().map_or(80, |(cols, _)| cols);
        let rows = super::screen_rows(&super::working_line("claude", 0, 0), cols);
        let _ = write!(out, "\x1b[{rows}A\r\x1b[J");
        let _ = out.flush();
    }
}

pub(super) fn fixture_kind(prompt: &str) -> Option<&str> {
    let kind = prompt.strip_prefix("fake:permission ")?.trim();
    [
        "safe",
        "collapsed",
        "shared",
        "visible-script",
        "truncated",
        "gh-read",
        "gh-post",
        "master-push",
        "destructive",
        "question",
    ]
    .contains(&kind)
    .then_some(kind)
}

fn screen(kind: &str) -> String {
    if kind == "collapsed" {
        return "• Working (0s • esc to interrupt)\r\n\r\n• Queued follow-up inputs\r\n  ? 1 question\r\n    ⇧← to answer\r\n\r\n› Ask Codex to do anything\r\n\r\n  gpt-fake · fake codex\r\n  ? for shortcuts\r\n".into();
    }
    let title = match kind {
        "safe" => "Would you like to run the following command?\r\n\r\n$ CARGO_TARGET_DIR=./target cargo test permission_safe_probe --locked",
        "shared" => "Would you like to run the following command?\r\n\r\n$ printf permission-shared > ../shared/permission-probe",
        "visible-script" => "Would you like to run the following command?\r\n\r\n$ python3 -c 'from pathlib import Path; Path(\"permission-probe\").write_text(\"safe\")'",
        "truncated" => "Would you like to run the following command?\r\n\r\n$ python3 -c 'from pathlib import Path; ...'\r\n51 more lines hidden",
        "gh-read" => "Would you like to run the following command?\r\n\r\n$ gh api repos/example/project/pulls --method GET",
        "gh-post" => "Would you like to run the following command?\r\n\r\n$ gh api repos/example/project/issues --method POST -f title=publish",
        "master-push" => "Would you like to run the following command?\r\n\r\n$ git push origin HEAD:master",
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
        let mut rendered = if self.provider == "claude" {
            screen(kind)
                .replace(
                    "Would you like to run the following command?",
                    "Do you want to proceed?",
                )
                .replace("› 1.", "❯ 1.")
        } else {
            screen(kind)
        };
        let mut out = std::io::stdout();
        let _ = write!(out, "{rendered}");
        let _ = out.flush();
        let started = Instant::now();
        let mut selected = 1i32;
        let mut collapsed = kind == "collapsed";
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
                    Input::OpenQuestion if collapsed => {
                        let rows = rendered.matches("\r\n").count();
                        rendered = screen("safe");
                        collapsed = false;
                        let _ = write!(out, "\x1b[{rows}A\r\x1b[J{rendered}");
                        let _ = out.flush();
                    }
                    Input::Enter
                        if !collapsed
                            && (!["safe", "collapsed"].contains(&kind) || selected == 1) =>
                    {
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
        if !service_prompt(&self.provider, prompt) {
            return None;
        }
        let handler = std::env::var("BUS_PERMISSION_HANDLER_ID").ok()?;
        Some(
            match validate_service_launch().and_then(|()| service_decision(&handler, prompt)) {
                Ok(outcome) => format!("Permission fixture completed: {outcome}."),
                Err(error) => format!("Permission fixture failed: {error}"),
            },
        )
    }
}

fn validate_service_launch() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = super::settings_path(&args).ok_or("Service settings missing")?;
    let settings: Value = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let allow = settings["permissions"]["allow"]
        .as_array()
        .ok_or("Service allowlist missing")?;
    for rule in [
        "Read",
        "Glob",
        "Grep",
        "Bash(cat *)",
        "Bash(ls *)",
        "Bash(realpath *)",
        "Bash(grep *)",
        "Bash(head *)",
        "Bash(test *)",
    ] {
        if !allow.contains(&json!(rule)) {
            return Err(format!("Service inspection would prompt: missing {rule}"));
        }
    }
    if settings["permissions"]["defaultMode"] != "dontAsk"
        || !args
            .windows(2)
            .any(|a| a == ["--permission-mode", "dontAsk"])
        || !args
            .windows(2)
            .any(|a| a == ["--tools", "Read,Glob,Grep,Bash"])
    {
        return Err("Service unlisted work can still prompt".into());
    }
    let binary = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let prefix = crate::agents::providers::launch::profile::cli_prefix(&binary);
    for family in [
        "agent dialog",
        "agent choose",
        "agent answer",
        "agent permission-decision",
    ] {
        if !allow.contains(&json!(format!("Bash({prefix} {family} *)"))) {
            return Err(format!("Service protocol would prompt: {family}"));
        }
    }
    if !allow.contains(&json!(format!("Bash({prefix} state)"))) {
        return Err("Service state would prompt".into());
    }
    let mut trace = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("fake-permission-cli.ndjson")
        .map_err(|e| e.to_string())?;
    writeln!(trace, "{}", json!({"service_profile_validated":true,"approval_mode":"dontAsk","available_tools":"Read,Glob,Grep,Bash"}))
        .map_err(|e| e.to_string())
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
    decode_response(&output.stdout)
}

fn decode_response(stdout: &[u8]) -> Result<Value, String> {
    let response: crate::messaging::control::Response =
        serde_json::from_slice(stdout).map_err(|e| e.to_string())?;
    if !response.ok {
        return Err(response
            .error
            .map(|error| error.message)
            .unwrap_or_else(|| "CLI command failed".into()));
    }
    Ok(response.result)
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
    let handler_id = handler.parse::<u64>().map_err(|_| "Invalid handler ID")?;
    let assignment = state["permission_assignments"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["handler"].as_u64() == Some(handler_id) && a["token"].as_str() == Some(token))
        .ok_or("No matching active handler assignment")?;
    let worker = assignment["worker"].to_string();
    let observed = call(&["agent", "dialog", &worker])?;
    let text = observed["dialog"]["text"].as_str().unwrap_or_default();
    let (mut outcome, mut summary) = fixture_decision(text);
    if outcome == "approved" && !fixture_scope_valid(&state, assignment, text) {
        outcome = "escalated";
        summary = "fixture command lacks a canonical allowed root";
    }
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
    if text.contains("$ CARGO_TARGET_DIR=./target cargo test permission_safe_probe --locked") {
        ("approved", "safe worker test command")
    } else if text.contains("$ printf permission-shared > ../shared/permission-probe") {
        ("approved", "write inside explicit granted shared root")
    } else if text.contains(
        "$ python3 -c 'from pathlib import Path; Path(\"permission-probe\").write_text(\"safe\")'",
    ) {
        (
            "approved",
            "complete visible script writes inside worker root",
        )
    } else if text.contains("$ gh api repos/example/project/pulls --method GET") {
        (
            "approved",
            "read-only GitHub GET without fields or credentials",
        )
    } else if text.contains("51 more lines hidden")
        || text.contains("--method POST")
        || text.contains("HEAD:master")
    {
        (
            "escalated",
            "opaque script, GitHub mutation or protected master push",
        )
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

fn fixture_scope_valid(state: &Value, assignment: &Value, text: &str) -> bool {
    let Some(worker) = state["agents"]
        .as_array()
        .and_then(|agents| agents.iter().find(|a| a["id"] == assignment["worker"]))
    else {
        return false;
    };
    let Some(root) = worker["launch_cwd"].as_str().map(std::path::Path::new) else {
        return false;
    };
    if root.canonicalize().ok().as_deref() != Some(root) {
        return false;
    }
    if text.contains("permission-shared") {
        let Some(shared) = root
            .parent()
            .map(|p| p.join("shared"))
            .and_then(|p| p.canonicalize().ok())
        else {
            return false;
        };
        return worker["allowed_roots"].as_array().is_some_and(|grants| {
            grants.iter().any(|g| {
                g["path"]
                    .as_str()
                    .is_some_and(|p| std::path::Path::new(p) == shared)
            })
        });
    }
    true
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
    fn permission_fixture_claude_busy_frame_confirms_native_acceptance() {
        let mut screen = super::super::submitted("Permission assignment p8-123.").into_bytes();
        begin_work(&mut screen, true);
        let screen = String::from_utf8(screen).unwrap();
        let detection = crate::agents::manifest::detect_with_osc(
            crate::agents::AgentKind::Claude,
            crate::agents::manifest::DetectionInput {
                screen: &screen,
                osc_title: "",
                osc_progress: "",
            },
        );
        assert!(
            detection.visible_working,
            "native submit cannot confirm this frame: {screen}"
        );
    }

    #[test]
    fn permission_fixture_cli_decodes_real_response_envelope() {
        let result = json!({"permission_assignments":[{"worker":6,"handler":7,"token":"p8-123"}]});
        let response = crate::messaging::control::Response::success("test", result.clone());
        let stdout = serde_json::to_vec(&response).unwrap();
        assert_eq!(decode_response(&stdout).unwrap(), result);
    }

    #[test]
    fn permission_fixture_cli_rejects_failed_response_envelope() {
        let response = crate::messaging::control::Response::failure(
            "test",
            "stale_assignment",
            "Lease expired",
        );
        let stdout = serde_json::to_vec(&response).unwrap();
        assert_eq!(decode_response(&stdout).unwrap_err(), "Lease expired");
    }

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
    fn permission_fixture_coverage_has_safe_and_excluded_native_screens() {
        for (kind, outcome) in [
            ("shared", "approved"),
            ("visible-script", "approved"),
            ("truncated", "escalated"),
            ("gh-read", "approved"),
            ("gh-post", "escalated"),
            ("master-push", "escalated"),
        ] {
            assert_eq!(fixture_kind(&format!("fake:permission {kind}")), Some(kind));
            let dialog = crate::agents::dialog::parse(&screen(kind)).unwrap();
            assert_eq!(fixture_decision(&dialog.text).0, outcome, "{kind}");
        }
    }

    #[test]
    fn permission_fixture_collapsed_codex_question_is_native_pending() {
        assert_eq!(fixture_kind("fake:permission collapsed"), Some("collapsed"));
        assert!(crate::agents::dialog::codex_question_pending(&screen(
            "collapsed"
        )));
        assert!(crate::agents::dialog::parse(&screen("collapsed")).is_none());
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
                    let result = match args {
                    ["state"] => json!({"agents":[{"id":6,"launch_cwd":std::env::current_dir().unwrap().canonicalize().unwrap()}],"permission_assignments":[{"worker":6,"handler":7,"token":"p8-123"}]}),
                    ["agent", "dialog", "6"] => json!({"dialog":{"text":text},"fingerprint":"d1.observed"}),
                    ["agent", "choose", ..] => json!({"outcome":"closed"}),
                    ["agent", "permission-decision", ..] => json!({"recorded":true}),
                    _ => panic!("unexpected control operation: {args:?}"),
                    };
                    let response = crate::messaging::control::Response::success("fixture", result);
                    decode_response(&serde_json::to_vec(&response).unwrap())
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
