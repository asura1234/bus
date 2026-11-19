//! Drive the real key path, CLI preflight and whole-session handoff in scratch Bus.
use super::*;
use std::time::{Duration, Instant};

fn read_json(path: &Path) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}

fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !predicate() {
        assert!(Instant::now() < deadline, "restart condition timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn fake_cargo(session: &Session, fail: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = session_dir(&session.0).join("bin");
    let mark = bin.join("cargo-called");
    let cargo = bin.join("cargo");
    // A successful stub reuses this exact test binary; never rebuild a live binary.
    std::fs::write(
        &cargo,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nwhile [ -f '{}' ]; do /bin/sleep 0.05; done\necho hotkey-build-fixture >&2\nexit {}\n",
            mark.display(),
            bin.join("cargo-hold").display(),
            if fail { 1 } else { 0 }
        ),
    )
    .unwrap();
    std::fs::set_permissions(cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    mark
}

#[test]
fn ctrl_r_starts_the_existing_whole_session_restart_handoff() {
    let session = Session::hold(session_name("hotkey"));
    let (code, started) = tui(
        &session.0,
        &["start", "--yolo", "--run-dir", run_dir().to_str().unwrap()],
    );
    assert_eq!(code, 0, "{started}");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    let status = session.ok(&["status"]);
    let data = PathBuf::from(status["data_dir"].as_str().unwrap());
    let work = session_dir(&session.0).join("work");
    let (code, added) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "restart-worker",
            "--provider",
            "claude",
            "--pwd",
            work.to_str().unwrap(),
            "--consent-hooks",
        ],
    ));
    assert_eq!(code, 0, "{added}");
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "20s",
    ]);
    let provider_session =
        bus_state(&session)["agents"][0]["runtime_identity"]["session_id"].clone();
    assert!(provider_session.is_string(), "{provider_session}");
    let before = read_json(&data.join("restarts/tui.json"));
    assert!(before["process"]["pid"].is_number());
    let mark = fake_cargo(&session, false);
    session.ok(&["type", "retained restart draft"]);
    session.ok(&["press", "ctrl+r"]);
    wait_for(|| data.join("restarts/outcome.json").exists());
    let outcome = read_json(&data.join("restarts/outcome.json"));
    assert_eq!(outcome["outcome"], "restarted", "{outcome}");
    assert!(std::fs::read_to_string(mark).unwrap().contains("--locked"));
    let after = read_json(&data.join("restarts/tui.json"));
    assert_ne!(before["process"]["pid"], after["process"]["pid"]);
    let plan = read_json(&data.join("restarts/active.json"));
    assert_eq!(plan["dev"], true);
    assert_eq!(plan["yolo"], true);
    assert_eq!(plan["explicit_data_dir"], true);
    assert_eq!(plan["clear_data"], false);
    session.ok(&[
        "wait",
        "--text",
        "retained restart draft",
        "--timeout",
        "10s",
    ]);
    assert_eq!(session.ok(&["status"])["alive"], true);
    let (code, state) = parse(scratch_bus(&session, &["state"]));
    assert_eq!(code, 0, "{state}");
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "20s",
    ]);
    assert_eq!(
        bus_state(&session)["agents"][0]["runtime_identity"]["session_id"],
        provider_session
    );
}

#[test]
fn failed_ctrl_r_build_keeps_the_session_and_composer_running() {
    let session = Session::start("hotfail");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    let status = session.ok(&["status"]);
    let data = PathBuf::from(status["data_dir"].as_str().unwrap());
    let mark = fake_cargo(&session, true);
    let hold = mark.with_file_name("cargo-hold");
    std::fs::write(&hold, b"hold build until UI has been exercised").unwrap();
    session.ok(&["type", "keep draft"]);
    session.ok(&["press", "ctrl+r"]);
    wait_for(|| mark.exists());
    session.ok(&["type", " during build"]);
    session.ok(&["expect", "--text", "keep draft during build"]);
    session.ok(&["press", "ctrl+r"]);
    assert_eq!(
        std::fs::read_to_string(&mark)
            .unwrap()
            .lines()
            .filter(|line| *line == "--locked")
            .count(),
        1,
        "repeated keypresses must not start concurrent builds"
    );
    std::fs::remove_file(hold).unwrap();
    wait_for(|| {
        std::fs::read_to_string(data.join("herdr-config/sessions/bus/herdr-client.log"))
            .unwrap_or_default()
            .contains("hotkey-build-fixture")
    });
    assert!(!data.join("restarts/handoff.json").exists());
    assert_eq!(session.ok(&["status"])["alive"], true);
    session.ok(&["type", " after failure"]);
    session.ok(&["expect", "--text", "keep draft during build after failure"]);
    assert_eq!(parse(scratch_bus(&session, &["state"])).0, 0);
}

#[test]
fn non_dev_ctrl_r_refuses_like_the_cli_and_keeps_running() {
    let session = Session::hold(session_name("hotnodev"));
    let (code, status) = tui(
        &session.0,
        &[
            "start",
            "--no-dev",
            "--run-dir",
            run_dir().to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{status}");
    let data = PathBuf::from(status["data_dir"].as_str().unwrap());
    let mark = fake_cargo(&session, true);
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    session.ok(&["type", "nondev draft"]);
    session.ok(&["press", "ctrl+r"]);
    let log = data.join("restarts/log.jsonl");
    wait_for(|| {
        std::fs::read_to_string(&log).is_ok_and(|text| text.contains("\"outcome\":\"refused\""))
    });
    let text = std::fs::read_to_string(&log).unwrap();
    let outcome: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(outcome["outcome"], "refused");
    assert_eq!(
        outcome["reason"],
        "dev tool: run `bus --dev restart --confirm`"
    );
    assert!(!mark.exists(), "non-dev must refuse before building");
    assert_eq!(session.ok(&["status"])["alive"], true);
    session.ok(&["type", " survives"]);
    session.ok(&["expect", "--text", "nondev draft survives"]);
    assert_eq!(parse(scratch_bus(&session, &["state"])).0, 0);
    // The driver must preserve normal mode as well when it relaunches its Bus.
    session.ok(&["restart"]);
    let stamp = read_json(&data.join("restarts/build.json"));
    assert_eq!(stamp["dev"], false, "{stamp}");
    session.ok(&["expect", "--text", "nondev draft survives"]);
    let stopped = session.ok(&["stop"]);
    assert_eq!(
        stopped["quit"], true,
        "normal-mode cleanup must save and quit: {stopped}"
    );
}

#[test]
fn neighbouring_ctrl_q_still_saves_the_draft_and_quits() {
    let session = Session::start("hotquit");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    let status = session.ok(&["status"]);
    let data = PathBuf::from(status["data_dir"].as_str().unwrap());
    session.ok(&["type", "saved by ctrl q"]);
    session.ok(&["press", "ctrl+q"]);
    wait_for(|| tui(&session.0, &["status"]).1["alive"] == false);
    let state = std::fs::read_to_string(data.join("state.json")).unwrap();
    assert!(state.contains("saved by ctrl q"), "{state}");
    assert!(!data.join("restarts/handoff.json").exists());
}
