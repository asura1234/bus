//! `bus --dev tui` against the built binary: a scratch Bus driven only through
//! the driver CLI, and the guards that keep it away from every other Bus.
// Only the dev variant has the driver; the gate runs this target in its
// dev-tools lane, and an explicit `--test '*'` must still build without it.
// Sessions start only on macOS, where the effect recorder covers Bus.
#![cfg(all(target_os = "macos", feature = "dev-tools"))]

#[path = "../../src/utils/tests/live_bus_env_test.rs"]
mod live_bus_env;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "native_restart_test.rs"]
mod native_restart;

#[path = "startup_test.rs"]
mod startup;

#[test]
fn reply_delivery_survives_actual_server_restart_mid_request() {
    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tools/acceptance/reply_restart_test.py"
        ))
        .args(["--binary", env!("CARGO_BIN_EXE_bus")])
        .output()
        .expect("run isolated reply restart scenario");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Each test drives its own session; names stay unique across test processes.
fn session_name(tag: &str) -> String {
    format!("it{}-{tag}", std::process::id() % 100_000)
}

fn run_dir() -> PathBuf {
    std::env::temp_dir().join(format!("bus-tui-it-runs-{}", std::process::id()))
}

/// Each session's own driver root: no other driver on the machine, and no
/// other test however the runner schedules them, counts against its cap.
fn root(name: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/bti-{name}"))
}

fn session_dir(name: &str) -> PathBuf {
    root(name).join(name)
}

/// `bus` for session NAME with only this suite's driver settings: no
/// inherited session.
fn bus(name: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bus"));
    // cargo builds the driver into target/debug; allow it for this suite only.
    command.env("BUS_TUI_ALLOW_TARGET_DEBUG", "1");
    command.env("BUS_TUI_ROOT", root(name));
    // The host inherits this. A runner SIGKILL never runs Drop or the panic
    // hook; the host reaps the scratch Bus when this pid is gone.
    command.env("BUS_TUI_REAP_WITH", std::process::id().to_string());
    for key in [
        "BUS_DATA_DIR",
        "BUS_SESSION_ID",
        "HERDR_SOCKET_PATH",
        "BUS_TUI_SESSION",
    ] {
        command.env_remove(key);
    }
    command
}

/// `tui bus ARGS` against a session's scratch Bus.
fn scratch_bus(session: &Session, args: &[&str]) -> Output {
    bus(&session.0)
        .args(["--dev", "tui", "bus", "--name", &session.0])
        .args(args)
        .output()
        .unwrap()
}

fn tui(name: &str, args: &[&str]) -> (i32, Value) {
    let output = tui_raw(name, args, &[]);
    parse(output)
}

fn tui_raw(name: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = bus(name);
    command
        .args(["--dev", "tui"])
        .args(args)
        .args(["--name", name]);
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("run bus --dev tui")
}

fn parse(output: Output) -> (i32, Value) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().last().unwrap_or("null");
    let value = serde_json::from_str(line).unwrap_or(Value::Null);
    (output.status.code().unwrap_or(-1), value)
}

/// Stops the session even when an assertion fails mid-test.
struct Session(String);

thread_local! {
    static SESSIONS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn install_reap_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Only this thread's sessions. A process-wide sweep would kill
            // scratch Buses that other tests still own.
            let names = SESSIONS.with(|slot| slot.borrow().clone());
            for name in &names {
                force_reap(name);
            }
            previous(info);
        }));
    });
}

fn kill_pid(pid: u32) {
    if pid <= 1 || pid == std::process::id() {
        return;
    }
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
}

fn pids_under(dir: &Path) -> Vec<u32> {
    if !dir.exists() {
        return Vec::new();
    }
    let Ok(output) = Command::new("lsof")
        .args(["-t", "+D"])
        .arg(dir)
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let me = std::process::id();
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter_map(|pid| pid.parse().ok())
        .filter(|pid| *pid > 1 && *pid != me)
        .collect()
}

fn recorded_pids(name: &str) -> Vec<u32> {
    let Ok(text) = std::fs::read_to_string(session_dir(name).join("session.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let mut pids = Vec::new();
    if let Some(pid) = value.get("host_pid").and_then(Value::as_u64) {
        pids.push(pid as u32);
    }
    if let Some(pid) = value.get("bus_pid").and_then(Value::as_u64) {
        pids.push(pid as u32);
    }
    pids
}

/// SIGKILL the host, the scratch Bus, and anything still using the private root.
/// Used when `tui stop` did not return and from the panic hook, which runs
/// before destructors.
fn force_reap(name: &str) {
    for pid in recorded_pids(name) {
        kill_pid(pid);
    }
    for dir in [root(name), session_dir(name)] {
        for pid in pids_under(&dir) {
            kill_pid(pid);
        }
    }
}

fn stop_bounded(name: &str) {
    let mut command = bus(name);
    command.args(["--dev", "tui", "stop", "--name", name]);
    let Ok(mut child) = command.spawn() else {
        return;
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

impl Session {
    /// Register before the start command returns, so a later assert still reaps.
    fn hold(name: String) -> Self {
        install_reap_hook();
        SESSIONS.with(|slot| slot.borrow_mut().push(name.clone()));
        Self(name)
    }

    fn start(tag: &str) -> Self {
        let session = Self::hold(session_name(tag));
        let runs = run_dir();
        let (code, status) = tui(
            &session.0,
            &[
                "start",
                "--size",
                "140x40",
                "--run-dir",
                runs.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 0, "start failed: {status}");
        assert_eq!(status["alive"], true);
        session
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, value) = tui(&self.0, args);
        assert_eq!(code, 0, "{args:?} failed: {value}");
        value
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        stop_bounded(&self.0);
        force_reap(&self.0);
        let name = self.0.clone();
        SESSIONS.with(|slot| slot.borrow_mut().retain(|registered| registered != &name));
        // A stopped session leaves no root behind.
        let _ = std::fs::remove_file(root(&self.0).join(".start.lock"));
        let _ = std::fs::remove_dir(root(&self.0));
    }
}

fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn wait_child(child: &mut std::process::Child, limit: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.try_wait().ok().flatten().is_some()
}

#[test]
fn a_panic_kills_only_that_threads_scratch_process() {
    let kept_name = session_name("keep");
    let kept_dir = session_dir(&kept_name);
    std::fs::create_dir_all(&kept_dir).expect("kept dir");
    let mut kept = Command::new("/bin/sleep")
        .arg("30")
        .current_dir(&kept_dir)
        .spawn()
        .expect("kept sleep");
    let kept_pid = kept.id();
    let kept_session = Session::hold(kept_name);

    let doomed_name = session_name("doom");
    let doomed_dir = session_dir(&doomed_name);
    std::fs::create_dir_all(&doomed_dir).expect("doomed dir");
    let joined = std::thread::spawn(move || {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .current_dir(&doomed_dir)
            .spawn()
            .expect("doomed sleep");
        let pid = child.id();
        let _session = Session::hold(doomed_name);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !pids_under(&doomed_dir).contains(&pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            pids_under(&doomed_dir).contains(&pid),
            "lsof missed the scratch process"
        );
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            panic!("injected scratch leak");
        }));
        let exited = wait_child(&mut child, std::time::Duration::from_secs(2));
        (caught.is_err(), exited)
    })
    .join()
    .expect("panic thread");
    assert!(joined.0, "panic was not raised");
    assert!(joined.1, "panic left the scratch process running");
    assert!(
        process_alive(kept_pid),
        "a panic on another thread killed this thread's scratch process"
    );
    let _ = kept.kill();
    let _ = kept.wait();
    drop(kept_session);
}

#[test]
fn stop_kills_processes_left_after_the_host_dies() {
    let session = Session::start("reap");
    let dir = session_dir(&session.0);
    let claim: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("bus-tui-session")).expect("claim"))
            .expect("claim json");
    let token = claim["token"].as_str().expect("token").to_owned();
    let info: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("session.json")).expect("session info"),
    )
    .expect("session info json");
    let host_pid = info["host_pid"].as_u64().expect("host pid") as u32;
    let bus_pid = info["bus_pid"].as_u64().expect("bus pid") as u32;
    let mut orphan = Command::new("/bin/sleep")
        .arg("60")
        .current_dir(&dir)
        .env("BUS_TUI_OWNER", &token)
        .spawn()
        .expect("orphan");
    let seen = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !pids_under(&dir).contains(&orphan.id()) && std::time::Instant::now() < seen {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        pids_under(&dir).contains(&orphan.id()),
        "lsof missed the orphan"
    );
    kill_pid(host_pid);
    let host_gone = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while process_alive(host_pid) && std::time::Instant::now() < host_gone {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut stop = bus(&session.0);
    stop.args(["--dev", "tui", "stop", "--name", &session.0]);
    let mut stop = stop.spawn().expect("spawn stop");
    let stop_deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while stop.try_wait().ok().flatten().is_none() && std::time::Instant::now() < stop_deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if stop.try_wait().ok().flatten().is_none() {
        let _ = stop.kill();
        let _ = stop.wait();
        panic!("tui stop did not return after the host was killed");
    }
    assert!(
        wait_child(&mut orphan, std::time::Duration::from_secs(2)),
        "stop left a scratch process behind"
    );
    let bus_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while process_alive(bus_pid) && std::time::Instant::now() < bus_deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        !process_alive(bus_pid),
        "scratch bus {bus_pid} survived the host"
    );
}

fn bus_state(session: &Session) -> Value {
    let output = scratch_bus(session, &["state"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<Value>(stdout.trim()).unwrap()["result"].clone()
}

#[test]
fn scratch_restart_preserves_unread_dot_and_trace_then_keeps_seen_rooms_clear() {
    let name = session_name("dot");
    let session = Session::hold(name.clone());
    let (code, started) = parse(tui_raw(
        &name,
        &[
            "start",
            "--size",
            "140x40",
            "--run-dir",
            run_dir().to_str().unwrap(),
        ],
        &[("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")],
    ));
    assert_eq!(code, 0, "{started}");
    let original = session.ok(&["status"]);
    let work = session_dir(&session.0).join("work");
    let (code, _) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "dot-worker",
            "--provider",
            "claude",
            "--pwd",
            work.to_str().unwrap(),
            "--consent-hooks",
        ],
    ));
    assert_eq!(code, 0);
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "20s",
    ]);
    let (code, bootstrap) = parse(scratch_bus(
        &session,
        &[
            "send",
            "--room",
            "bus",
            "--to",
            "dot-worker",
            "--text",
            "bootstrap restart fixture",
        ],
    ));
    assert_eq!(code, 0, "{bootstrap}");
    let bootstrap_message = bootstrap["result"]["message_id"].to_string();
    session.ok(&["wait", "--message", &bootstrap_message, "--timeout", "30s"]);
    let (code, _) = parse(scratch_bus(&session, &["room", "create", "unseen"]));
    assert_eq!(code, 0);
    session.ok(&["wait", "--text", "# unseen", "--timeout", "10s"]);
    let original_session =
        bus_state(&session)["agents"][0]["runtime_identity"]["session_id"].clone();
    for room in ["unseen", "master"] {
        let (code, sent) = parse(scratch_bus(
            &session,
            &[
                "send",
                "--room",
                room,
                "--as",
                "dot-worker",
                "--to",
                "you",
                "--text",
                "DOT persisted",
            ],
        ));
        assert_eq!(code, 0, "{sent}");
    }
    let trace = PathBuf::from(original["run_dir"].as_str().unwrap()).join("trace.ndjson");
    let before = std::fs::metadata(&trace).unwrap().len();
    let restarted = session.ok(&["restart"]);
    assert_eq!(restarted["old_bus_pid"], original["bus_pid"]);
    assert_ne!(restarted["bus_pid"], original["bus_pid"]);
    assert_eq!(restarted["data_dir"], original["data_dir"]);
    assert!(std::fs::metadata(&trace).unwrap().len() > before);
    session.ok(&[
        "wait",
        "--state",
        "agents.0.status == idle",
        "--timeout",
        "10s",
    ]);
    assert_eq!(
        bus_state(&session)["agents"][0]["runtime_identity"]["session_id"],
        original_session
    );
    let (code, sent) = parse(scratch_bus(
        &session,
        &[
            "send",
            "--room",
            "bus",
            "--to",
            "dot-worker",
            "--text",
            "after restart",
        ],
    ));
    assert_eq!(code, 0, "{sent}");
    let message = sent["result"]["message_id"].to_string();
    let replied = session.ok(&["wait", "--message", &message, "--timeout", "30s"]);
    assert_eq!(
        replied["message"]["result"]["requests"][0]["reply"]["text"],
        "fake reply: after restart"
    );
    for room in bus_state(&session)["rooms"].as_array().unwrap() {
        if matches!(room["name"].as_str(), Some("unseen" | "MASTER")) {
            assert_eq!(room["unseen_to_developer"], true);
        }
    }
    session.ok(&["wait", "--text", "●", "--rows", "1-7", "--timeout", "10s"]);
    session.ok(&["click", "--hit", "room:unseen"]);
    session.ok(&["click", "--hit", "room:MASTER"]);
    session.ok(&["click", "--hit", "room:bus"]);
    session.ok(&["press", "ctrl+q"]);
    session.ok(&["restart"]);
    for room in bus_state(&session)["rooms"].as_array().unwrap() {
        assert_eq!(room["unseen_to_developer"], false);
    }
    session.ok(&[
        "wait",
        "--text",
        "●",
        "--gone",
        "--rows",
        "1-7",
        "--timeout",
        "10s",
    ]);
}

fn assert_client_death_stops_scratch_tree(signal: i32, tag: &str) {
    use std::time::{Duration, Instant};
    let session = Session::start(tag);
    let work = session_dir(&session.0).join("work");
    let added = scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "cleanup",
            "--provider",
            "claude",
            "--pwd",
            work.to_str().unwrap(),
        ],
    );
    assert!(added.status.success(), "{added:?}");
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "20s",
    ]);
    let client = session.ok(&["status"])["bus_pid"].as_u64().unwrap() as u32;
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .unwrap();
    assert!(output.status.success());
    let rows: Vec<(u32, u32)> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            (
                fields.next().unwrap().parse().unwrap(),
                fields.next().unwrap().parse().unwrap(),
            )
        })
        .collect();
    let mut owned = vec![client];
    loop {
        let children: Vec<_> = rows
            .iter()
            .filter(|(pid, parent)| owned.contains(parent) && !owned.contains(pid))
            .map(|(pid, _)| *pid)
            .collect();
        if children.is_empty() {
            break;
        }
        owned.extend(children);
    }
    assert!(
        owned.len() >= 4,
        "expected client, server, pane shell and fake provider: {owned:?}"
    );
    assert_eq!(unsafe { libc::kill(client as i32, signal) }, 0);
    let deadline = Instant::now() + Duration::from_secs(12);
    let survivors = loop {
        let survivors: Vec<_> = owned
            .iter()
            .copied()
            .filter(|pid| unsafe { libc::kill(*pid as i32, 0) == 0 })
            .collect();
        if survivors.is_empty() || Instant::now() >= deadline {
            break survivors;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // Assert before Session::Drop invokes the driver's fallback teardown.
    assert!(
        survivors.is_empty(),
        "Bus-owned processes survived client signal {signal}: {survivors:?}"
    );
}

#[test]
fn client_sigkill_stops_server_and_provider_panes() {
    assert_client_death_stops_scratch_tree(libc::SIGKILL, "kill");
}

#[test]
fn client_sigterm_stops_server_and_provider_panes() {
    assert_client_death_stops_scratch_tree(libc::SIGTERM, "term");
}

#[test]
fn a_room_and_a_message_round_trip_through_real_keys_and_clicks() {
    let session = Session::start("flow");
    // The fresh scratch Bus draws its default room.
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);

    // Create a room by keys.
    session.ok(&["press", "ctrl+shift+r"]);
    session.ok(&["wait", "--text", "Room name"]);
    session.ok(&["type", "review"]);
    let entered = session.ok(&["press", "enter"]);
    assert!(entered["frame"].is_object(), "{entered}");
    session.ok(&["wait", "--state", "rooms.*.name == review"]);

    // Focus the default room by clicking its sidebar entry.
    let clicked = session.ok(&["click", "--hit", "room:bus"]);
    assert_eq!(clicked["at"][0], 5);
    session.ok(&["wait", "--state", "visible_room == 1"]);

    // A fake Claude agent answers through Bus's real hooks.
    let work = session_dir(&session.0).join("work");
    let work = work.to_str().unwrap();
    let status = bus(&session.0)
        .args(["--dev", "tui", "bus", "--name", &session.0])
        .args([
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "a1",
            "--provider",
            "claude",
            "--pwd",
            work,
        ])
        .status()
        .unwrap();
    assert!(status.success());
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "20s",
    ]);

    // Send through the composer: choose the agent, type, Enter.
    session.ok(&["click", "--text", "Choose agents"]);
    session.ok(&["wait", "--text", "[ ] a1"]);
    session.ok(&["click", "--text", "[ ] a1"]);
    session.ok(&["press", "esc"]);
    session.ok(&["click", "--text", "Message selected agents"]);
    session.ok(&["type", "hello from the driver"]);
    session.ok(&["press", "enter"]);
    let reply = session.ok(&[
        "wait",
        "--text",
        "fake reply: hello from the driver",
        "--timeout",
        "20s",
    ]);
    assert_eq!(reply["matches"].as_array().unwrap().len(), 1);
    let rooms = bus_state(&session)["rooms"].as_array().unwrap().len();
    assert_eq!(rooms, 3);

    // A drag over the reply copies it through OSC 52 into the driver.
    let found = session.ok(&["find", "fake reply"]);
    let row = found["matches"][0]["row"].as_u64().unwrap().to_string();
    let col = found["matches"][0]["col"].as_u64().unwrap();
    let dragged = session.ok(&["drag", &row, &col.to_string(), &row, &(col + 9).to_string()]);
    assert_eq!(dragged["copied"][0], "fake reply");
    // "fake" is all base64 letters; the copy still comes back as shown.
    let dragged = session.ok(&["drag", &row, &col.to_string(), &row, &(col + 3).to_string()]);
    assert_eq!(dragged["copied"][0], "fake");

    // A multi-line message reaches the fake Claude as one prompt.
    let sent = scratch_bus(
        &session,
        &[
            "send",
            "--room",
            "bus",
            "--to",
            "a1",
            "--text",
            "line one\nline two",
        ],
    );
    let receipt = String::from_utf8_lossy(&sent.stdout).to_string();
    assert!(sent.status.success(), "{receipt}");
    let receipt: Value = serde_json::from_str(receipt.trim()).unwrap();
    let id = receipt["result"]["message_id"].to_string();
    let settled = session.ok(&["wait", "--message", &id, "--timeout", "30s"]);
    assert!(
        settled
            .to_string()
            .contains("fake reply: line one\\nline two"),
        "the reply should answer the whole two-line prompt: {settled}"
    );

    let resized = session.ok(&["resize", "100x30"]);
    assert_eq!(resized["size"][0], 100);
    let snapshot = session.ok(&["snapshot"]);
    assert_eq!(snapshot["size"][1], 30);

    let (code, stopped) = tui(&session.0, &["stop"]);
    assert_eq!(code, 0, "{stopped}");
    let run = PathBuf::from(stopped["run_dir"].as_str().unwrap());
    assert!(run.join("trace.ndjson").is_file());
    assert!(run.join("pty.cast").is_file());
    assert!(!session_dir(&session.0).exists());
}

#[test]
fn a_fake_codex_agent_gets_room_prompts_and_replies() {
    room_prompts_reach_a_fake("codex", ".codex/hooks.json");
}

#[test]
fn a_fake_cursor_agent_gets_room_prompts_and_replies() {
    room_prompts_reach_a_fake("cursor", ".cursor/hooks.json");
}

/// Adds a fake `provider` worker and sends it room messages: each must be
/// typed into its composer and settle with its reply.
fn room_prompts_reach_a_fake(provider: &str, hooks: &str) {
    let session = Session::start(provider);
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    let work = session_dir(&session.0).join("work");
    let added = scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "c1",
            "--provider",
            provider,
            "--pwd",
            work.to_str().unwrap(),
            "--consent-hooks",
        ],
    );
    assert!(added.status.success(), "{added:?}");
    assert!(work.join(hooks).is_file());
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "20s",
    ]);

    let send = |text: &str| -> String {
        let sent = scratch_bus(
            &session,
            &["send", "--room", "bus", "--to", "c1", "--text", text],
        );
        let receipt = String::from_utf8_lossy(&sent.stdout).to_string();
        assert!(sent.status.success(), "{receipt}");
        let receipt: Value = serde_json::from_str(receipt.trim()).unwrap();
        receipt["result"]["message_id"].to_string()
    };

    // Bus types each prompt into the fake's composer; its hooks reply.
    for text in [format!("hello {provider}"), "hello again".into()] {
        let id = send(&text);
        let settled = session.ok(&["wait", "--message", &id, "--timeout", "30s"]);
        assert!(
            settled.to_string().contains(&format!("fake reply: {text}")),
            "the fake {provider} should answer {text:?}: {settled}"
        );
    }

    // A message sent while the fake works is typed into its running
    // turn, whose one reply answers both.
    let lead = send("fake:work 6 long task");
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == working",
        "--timeout",
        "20s",
    ]);
    let steer = send("steer in");
    for id in [&lead, &steer] {
        let settled = session.ok(&["wait", "--message", id, "--timeout", "40s"]);
        assert!(
            settled
                .to_string()
                .contains("fake reply: fake:work 6 long task\\nsteer in"),
            "the steer should join the running turn's reply: {settled}"
        );
    }
}

#[test]
fn the_driver_refuses_to_touch_a_running_session() {
    let session = Session::start("guard");
    // The same name again would be the running session: refused.
    let (code, refused) = tui(&session.0, &["start"]);
    assert_eq!(code, 1, "{refused}");
    assert_eq!(refused["error"]["code"], "refused");

    // An inherited BUS_DATA_DIR pointing at the target is refused.
    let other = session_name("env");
    let target = session_dir(&other).join("data");
    let (code, refused) = parse(tui_raw(
        &other,
        &["start"],
        &[("BUS_DATA_DIR", target.to_str().unwrap())],
    ));
    assert_eq!(code, 1, "{refused}");
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("BUS_DATA_DIR"));

    // A directory a live Bus serves is refused, even at the driver's own path.
    let foreign = session_name("live");
    let data = session_dir(&foreign).join("data");
    std::fs::create_dir_all(&data).unwrap();
    // A root the driver made, so the live Bus is what gets refused.
    std::fs::write(root(&foreign).join(".start.lock"), b"").unwrap();
    let socket = data.join("control.sock");
    let _live = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let (code, refused) = tui(&foreign, &["start"]);
    assert_eq!(code, 1, "{refused}");
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("live Bus"));
    let _ = std::fs::remove_dir_all(root(&foreign));

    // A second host for the running session, wrong token or right, is
    // refused before it touches anything.
    let marker = session_dir(&session.0).join("bus-tui-session");
    let claim: Value = serde_json::from_str(&std::fs::read_to_string(marker).unwrap()).unwrap();
    let runs = run_dir();
    for token in ["wrong", claim["token"].as_str().unwrap()] {
        let host = bus(&session.0)
            .args(["--dev", "tui", "__host", "--name", &session.0])
            .args(["--size", "80x24", "--token", token, "--run-dir"])
            .arg(&runs)
            .output()
            .unwrap();
        assert_eq!(host.status.code(), Some(1), "{host:?}");
    }
    assert!(!session_dir(&session.0).join("host.error").exists());

    // The running session is untouched by all of the above.
    assert_eq!(session.ok(&["status"])["alive"], true);
}

#[test]
fn exit_codes_follow_the_contract() {
    let missing = session_name("none");
    let (code, value) = tui(&missing, &["snapshot"]);
    assert_eq!(code, 3, "{value}");
    let (code, _) = tui(&missing, &["teleport"]);
    assert_eq!(code, 2);
    let (code, _) = tui(&missing, &["click"]);
    assert_eq!(code, 2);
    let session = Session::start("codes");
    let (code, timeout) = tui(
        &session.0,
        &["wait", "--text", "never shown", "--timeout", "300ms"],
    );
    assert_eq!(code, 1);
    assert!(timeout["excerpt"].is_array());
    let (code, _) = tui(&session.0, &["click", "--text", "never shown"]);
    assert_eq!(code, 1);
    // `bus tui` without --dev is refused.
    let plain = Command::new(env!("CARGO_BIN_EXE_bus"))
        .arg("tui")
        .output()
        .unwrap();
    assert_eq!(plain.status.code(), Some(2));
    let help = Command::new(env!("CARGO_BIN_EXE_bus"))
        .args(["--dev", "tui", "click", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&help.stdout).contains("### click"));
}

#[test]
fn stop_from_the_work_dir_keeps_a_relative_run_dir() {
    let name = session_name("rel");
    let session = Session::hold(name);
    let caller = std::env::temp_dir().join(format!("bus-tui-it-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&caller).unwrap();
    let caller = caller.canonicalize().unwrap();
    let started = bus(&session.0)
        .current_dir(&caller)
        .args([
            "--dev", "tui", "start", "--name", &session.0, "--size", "80x24",
        ])
        .args(["--run-dir", "runs"])
        .output()
        .unwrap();
    let (code, status) = parse(started);
    assert_eq!(code, 0, "{status}");
    // `stop` run from inside the session, as an agent there would.
    let stopped = bus(&session.0)
        .current_dir(session_dir(&session.0).join("work"))
        .args(["--dev", "tui", "stop", "--name", &session.0])
        .output()
        .unwrap();
    let (code, stopped) = parse(stopped);
    let run = PathBuf::from(stopped["run_dir"].as_str().unwrap_or_default());
    let kept = run.join("pty.cast").is_file();
    let _ = std::fs::remove_dir_all(&caller);
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(stopped["ok"], true, "{stopped}");
    assert!(run.starts_with(&caller), "{}", run.display());
    assert!(kept, "{} has no pty.cast after stop", run.display());
}

#[test]
fn waits_report_a_bus_that_exited() {
    let session = Session::start("dead");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    // A stability wait already running when Bus dies must not pass on its
    // frozen last screen.
    let name = session.0.clone();
    let stable =
        std::thread::spawn(move || tui(&name, &["wait", "--stable", "3s", "--timeout", "15s"]));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = scratch_bus(&session, &["stop"]);
    let (code, value) = stable.join().unwrap();
    assert_eq!(code, 3, "wait --stable after Bus exited: {value}");
    let (code, value) = tui(
        &session.0,
        &["wait", "--text", "never shown", "--timeout", "2s"],
    );
    assert_eq!(code, 3, "wait --text after Bus exited: {value}");
    for predicate in ["visible_room == 99", "visible_room != 1"] {
        let (code, value) = tui(
            &session.0,
            &["wait", "--state", predicate, "--timeout", "2s"],
        );
        assert_eq!(
            code, 3,
            "wait --state {predicate} after Bus exited: {value}"
        );
    }
}

#[test]
fn a_quit_reports_the_exit_status_of_bus() {
    let session = Session::start("quit");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    session.ok(&["press", "ctrl+q"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        let (_, status) = tui(&session.0, &["status"]);
        if status["alive"] == false || std::time::Instant::now() > deadline {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    // The first status without Bus already says how it exited, not only that
    // its terminal closed.
    assert!(
        status["exited"]
            .as_str()
            .unwrap_or_default()
            .starts_with("bus exited"),
        "{status}"
    );
}

#[test]
fn stop_reports_when_effect_recording_cannot_be_archived() {
    let session = Session::start("archive");
    let status = session.ok(&["status"]);
    let artifact_dir = PathBuf::from(status["run_dir"].as_str().unwrap());
    let recording = session_dir(&session.0).join("effects.ndjson");
    std::fs::write(
        &recording,
        b"{\"kind\":\"sound\",\"target\":\"fixture.aiff\",\"at_ms\":1}\n",
    )
    .unwrap();
    // A directory at the destination deterministically makes the copy fail.
    let destination = artifact_dir.join("effects.ndjson");
    std::fs::create_dir(&destination).unwrap();
    let (code, stopped) = tui(&session.0, &["stop"]);
    let kept = std::fs::read_to_string(&recording).unwrap_or_default();
    std::fs::remove_dir(&destination).unwrap();
    let _ = std::fs::remove_dir_all(session_dir(&session.0));
    assert_ne!(code, 0, "{stopped}");
    assert_eq!(stopped["error"]["code"], "effects_not_kept", "{stopped}");
    assert!(
        kept.contains("fixture.aiff"),
        "the original recording must survive a failed archive: {stopped}"
    );
}

#[test]
fn reads_still_work_after_a_quit() {
    let session = Session::start("aq");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    session.ok(&["press", "ctrl+q"]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while tui(&session.0, &["status"]).1["alive"] != false {
        assert!(std::time::Instant::now() < deadline, "Bus did not exit");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for read in [
        &["snapshot", "--plain"][..],
        &["find", "Stopping"][..],
        &["cells", "0", "0"][..],
        &["screenshot"][..],
        &["effects"][..],
        &["clipboard"][..],
    ] {
        session.ok(read);
    }
    let (code, value) = tui(&session.0, &["press", "enter"]);
    assert_eq!(code, 3, "input after quit: {value}");
    // The output Bus wrote while quitting, terminal restore included, is
    // already in pty.raw before `stop`.
    let run = PathBuf::from(
        session.ok(&["status"])["run_dir"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    let raw = std::fs::read(run.join("pty.raw")).unwrap();
    let has = |needle: &[u8]| raw.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"Stopping Bus"), "pty.raw lacks the quit message");
    assert!(
        has(b"\x1b[?1049l"),
        "pty.raw lacks the alternate-screen exit"
    );
}

#[test]
fn nth_counts_matches_from_one() {
    let session = Session::start("nth");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    let first = session.ok(&["find", "bus"])["matches"][0].clone();
    // `--nth 1` is the first match in reading order.
    let hovered = session.ok(&["hover", "--text", "bus", "--nth", "1"]);
    assert_eq!(hovered["at"][0], first["row"], "{hovered} vs {first}");
    let (code, value) = tui(&session.0, &["hover", "--text", "bus", "--nth", "99"]);
    assert_eq!(code, 1, "{value}");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--nth counts from 1"),
        "{value}"
    );
}

#[test]
fn stop_keeps_the_recorded_effects_in_the_run_dir() {
    let session = Session::start("ek");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    session.ok(&["fixture", "clipboard-image"]);
    session.ok(&["click", "--hit", "composer"]);
    session.ok(&["press", "ctrl+v"]);
    session.ok(&["wait", "--effect", "clipboard_image"]);
    let stopped = session.ok(&["stop"]);
    let kept = PathBuf::from(stopped["run_dir"].as_str().unwrap()).join("effects.ndjson");
    let text = std::fs::read_to_string(&kept).unwrap_or_default();
    assert!(
        text.contains("clipboard_image"),
        "{}: {text:?}",
        kept.display()
    );
}

/// Attaches the session's `notes.txt` fixture the way the skill guide says:
/// Ctrl+O, clear the form's `~/`, type the absolute path, Enter.
fn attach_notes_with_the_file_form(session: &Session) -> String {
    let fixture = session.ok(&["fixture", "file", "--file", "notes.txt"]);
    let path = fixture["path"].as_str().unwrap().to_owned();
    session.ok(&["press", "ctrl+o"]);
    session.ok(&["wait", "--text", "Local file path"]);
    session.ok(&["press", "ctrl+a"]);
    session.ok(&["press", "ctrl+k"]);
    session.ok(&["type", &path]);
    session.ok(&["press", "enter"]);
    session.ok(&[
        "wait",
        "--text",
        "Local file path",
        "--gone",
        "--timeout",
        "5s",
    ]);
    session.ok(&["wait", "--text", "[notes.txt", "--timeout", "5s"]);
    path
}

#[test]
fn hovering_a_draft_attachment_chip_shows_its_full_path() {
    let session = Session::start("hov");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    let path = attach_notes_with_the_file_form(&session);
    let hits = session.ok(&["hits"]);
    let names: Vec<&str> = hits["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|hit| hit["name"].as_str())
        .collect();
    // A draft chip is `remove-file:NAME`; `file:NAME` is a history attachment.
    assert!(names.contains(&"remove-file:notes.txt"), "{names:?}");
    assert!(!names.contains(&"file:notes.txt"), "{names:?}");
    session.ok(&["hover", "--hit", "remove-file:notes.txt"]);
    let dir = Path::new(&path).parent().unwrap().file_name().unwrap();
    let (code, value) = tui(
        &session.0,
        &[
            "wait",
            "--text",
            &format!("{}/notes.txt", dir.to_str().unwrap()),
            "--timeout",
            "3s",
        ],
    );
    assert_eq!(code, 0, "full path not shown after hover: {value}");
}

#[test]
fn the_file_form_attaches_a_typed_absolute_path() {
    let session = Session::start("pst");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    attach_notes_with_the_file_form(&session);
}

#[test]
fn pasting_an_absolute_path_into_the_composer_attaches_it() {
    let session = Session::start("psc");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    let fixture = session.ok(&["fixture", "file", "--file", "notes.txt"]);
    let path = fixture["path"].as_str().unwrap().to_owned();
    session.ok(&["click", "--hit", "composer"]);
    session.ok(&["paste", &path]);
    let (code, value) = tui(
        &session.0,
        &["wait", "--text", "[notes.txt", "--timeout", "5s"],
    );
    assert_eq!(code, 0, "composer paste of a path: {value}");
}

#[test]
fn the_scratch_bus_lists_its_own_home_not_the_developers() {
    let session = Session::start("home");
    let status = session.ok(&["status"]);
    let home = PathBuf::from(status["home"].as_str().unwrap());
    assert_eq!(home, session_dir(&session.0).join("home"), "{status}");
    assert_eq!(status["graphics"], "kitty", "the default: {status}");
    std::fs::write(home.join("scratch-home-marker.txt"), b"x").unwrap();
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    // The file form starts at `~/` and suggests what is there.
    session.ok(&["press", "ctrl+o"]);
    session.ok(&[
        "wait",
        "--text",
        "scratch-home-marker.txt",
        "--timeout",
        "5s",
    ]);
}

#[test]
fn start_takes_the_graphics_identity_and_refuses_unknown_ones() {
    let name = session_name("gfx");
    let session = Session::hold(name.clone());
    let runs = run_dir();
    let (code, status) = tui(
        &name,
        &[
            "start",
            "--graphics",
            "none",
            "--size",
            "100x30",
            "--run-dir",
            runs.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{status}");
    assert_eq!(session.ok(&["status"])["graphics"], "none");
    let (code, refused) = tui(&session_name("gfx2"), &["start", "--graphics", "iterm2"]);
    assert_eq!(code, 2, "{refused}");
}

#[test]
fn caller_home_stays_out_of_file_suggestions_and_agent_settings() {
    fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(files_under(&path));
            } else if path.is_file() {
                files.push((path.clone(), std::fs::read(path).unwrap()));
            }
        }
        files.sort();
        files
    }

    let name = session_name("prefs");
    let session = Session::hold(name.clone());
    let runs = run_dir();
    let caller_home = runs.join("caller-home-homeprefs");
    std::fs::create_dir_all(caller_home.join(".claude")).unwrap();
    std::fs::write(caller_home.join("caller-home-marker.txt"), b"private").unwrap();
    std::fs::write(
        caller_home.join(".claude/settings.json"),
        br#"{"statusLine":{"type":"command","command":"echo caller-home-must-not-be-read"}}"#,
    )
    .unwrap();
    let before = files_under(&caller_home);
    let mut start = bus(&name);
    start.env("HOME", &caller_home);
    start.env("CLAUDE_CONFIG_DIR", caller_home.join(".claude"));
    start.env("LC_TERMINAL", "iTerm2");
    start.env("LC_TERMINAL_VERSION", "3.7.3");
    start.args(["--dev", "tui", "start", "--name", &name, "--run-dir"]);
    start.arg(&runs);
    let (code, status) = parse(start.output().unwrap());
    assert_eq!(code, 0, "{status}");
    let scratch_home = session_dir(&session.0).join("home");
    assert_eq!(status["home"], scratch_home.to_str().unwrap());
    std::fs::write(scratch_home.join("scratch-home-marker.txt"), b"public").unwrap();
    session.ok(&["press", "ctrl+o"]);
    session.ok(&["wait", "--text", "scratch-home-marker.txt"]);
    assert!(!session
        .ok(&["snapshot"])
        .to_string()
        .contains("caller-home-marker"));
    session.ok(&["press", "esc"]);

    let added = bus(&session.0)
        .env("HOME", &caller_home)
        .env("CLAUDE_CONFIG_DIR", caller_home.join(".claude"))
        .args(["--dev", "tui", "bus", "--name", &session.0])
        .args([
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "home-agent",
            "--provider",
            "claude",
            "--pwd",
            "~",
        ])
        .output()
        .unwrap();
    assert!(added.status.success(), "{added:?}");
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "20s",
    ]);
    let state = bus_state(&session);
    let agents = state["agents"].as_array().unwrap();
    assert_eq!(
        agents[0]["cwd"],
        scratch_home.canonicalize().unwrap().to_str().unwrap(),
        "{state}"
    );
    let data = session_dir(&session.0).join("data");
    let saved: Vec<_> = files_under(&data)
        .into_iter()
        .filter(|(path, _)| path.file_name().unwrap() == "claude-statusline.json")
        .collect();
    assert_eq!(
        saved.len(),
        1,
        "statusline settings should have been captured"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&saved[0].1).unwrap(),
        serde_json::json!({})
    );
    let (code, stopped) = tui(&session.0, &["stop"]);
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(files_under(&caller_home), before, "caller HOME was written");
    std::fs::remove_dir_all(caller_home).unwrap();
}

#[test]
fn a_refused_root_override_leaves_the_directory_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let base = PathBuf::from(format!("/tmp/bus-tui-it-root{}", std::process::id()));
    let live = base.join("live-data");
    let foreign = base.join("foreign");
    for dir in [&live, &foreign] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("state.json"), b"{\"sentinel\":true}").unwrap();
    }
    let _listener = std::os::unix::net::UnixListener::bind(live.join("control.sock")).unwrap();
    let snapshot = |dir: &PathBuf| {
        let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        (mode, names)
    };
    let runs = run_dir();
    let cases = [
        // An inherited live Bus data dir as the root.
        (&live, vec![("BUS_DATA_DIR", live.to_str().unwrap())]),
        // A directory with someone else's files in it.
        (&foreign, vec![]),
    ];
    let mut results = Vec::new();
    for (dir, extra) in cases {
        let before = snapshot(dir);
        let mut env = vec![("BUS_TUI_ROOT", dir.to_str().unwrap())];
        env.extend(extra);
        let (code, reply) = parse(tui_raw(
            &session_name("root"),
            &["start", "--run-dir", runs.to_str().unwrap()],
            &env,
        ));
        results.push((dir.clone(), code, reply, before, snapshot(dir)));
    }
    let _ = std::fs::remove_dir_all(&base);
    for (dir, code, reply, before, after) in results {
        assert_eq!(code, 1, "{}: {reply}", dir.display());
        assert_eq!(reply["error"]["code"], "refused", "{reply}");
        assert_eq!(
            after,
            before,
            "{} was changed by a refused start",
            dir.display()
        );
    }
}

#[test]
fn a_root_inside_a_live_data_dir_is_refused_however_it_is_spelled() {
    use std::os::unix::fs::PermissionsExt;
    let base = PathBuf::from(format!("/tmp/bus-tui-it-alias{}", std::process::id()));
    let live = base.join("live-data");
    let root = live.join("driver");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(live.join("state.json"), b"{\"sentinel\":true}").unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(live.join("control.sock")).unwrap();
    // On macOS the canonical spelling is /private/tmp/…, the root's is /tmp/….
    let canonical = live.canonicalize().unwrap();
    let snapshot = || {
        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        let names: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        (mode, names)
    };
    let before = snapshot();
    let name = session_name("alias");
    let (code, reply) = parse(tui_raw(
        &name,
        &["start", "--run-dir", run_dir().to_str().unwrap()],
        &[
            ("BUS_TUI_ROOT", root.to_str().unwrap()),
            ("BUS_DATA_DIR", canonical.to_str().unwrap()),
        ],
    ));
    let after = snapshot();
    if code == 0 {
        let _ = tui_raw(
            &name,
            &["stop"],
            &[("BUS_TUI_ROOT", root.to_str().unwrap())],
        );
    }
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(code, 1, "{reply}");
    assert_eq!(reply["error"]["code"], "refused", "{reply}");
    assert_eq!(
        after, before,
        "start touched a live Bus data dir through an alias"
    );
}

#[test]
fn a_root_override_with_dot_components_is_refused_before_anything_is_written() {
    use std::os::unix::fs::PermissionsExt;
    let base = PathBuf::from(format!("/tmp/bus-tui-it-dots{}", std::process::id()));
    let live = base.join("live");
    let actual_root = live.join("d");
    std::fs::create_dir_all(&actual_root).unwrap();
    std::fs::set_permissions(&actual_root, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(live.join("state.json"), b"{\"sentinel\":true}").unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(live.join("control.sock")).unwrap();
    let missing = base.join("m");
    let snapshot = || {
        let mode = std::fs::metadata(&actual_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let names: Vec<_> = std::fs::read_dir(&actual_root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        (mode, names)
    };
    let before = snapshot();
    let canonical = live.canonicalize().unwrap();
    let mut results = Vec::new();
    // `m` is missing, so `m/..` cannot be resolved; and a relative root.
    for requested in [
        format!("{}/m/../live/d", base.display()),
        format!("{}/./live/d", base.display()),
        "relative/root".to_owned(),
    ] {
        let name = session_name("dots");
        let (code, reply) = parse(tui_raw(
            &name,
            &["start", "--run-dir", run_dir().to_str().unwrap()],
            &[
                ("BUS_TUI_ROOT", requested.as_str()),
                ("BUS_DATA_DIR", canonical.to_str().unwrap()),
            ],
        ));
        if code == 0 {
            let _ = tui_raw(&name, &["stop"], &[("BUS_TUI_ROOT", requested.as_str())]);
        }
        results.push((requested, code, reply, snapshot(), missing.exists()));
    }
    let _ = std::fs::remove_dir_all(&base);
    for (requested, code, reply, after, created) in results {
        assert_eq!(code, 1, "{requested}: {reply}");
        assert_eq!(reply["error"]["code"], "refused", "{requested}: {reply}");
        assert_eq!(after, before, "{requested} changed the live data dir");
        assert!(!created, "{requested} created its missing component");
    }
}

#[test]
fn named_hits_effects_fixtures_and_screenshots_work_on_a_scratch_bus() {
    let session = Session::start("layer3");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    let hits = session.ok(&["hits"]);
    let names: Vec<&str> = hits["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|hit| hit["name"].as_str())
        .collect();
    for wanted in ["new-room", "new-agent", "settings", "composer", "room:bus"] {
        assert!(names.contains(&wanted), "{wanted} missing from {names:?}");
    }

    // Settings by name; the toggle changes the setting and its own row.
    session.ok(&["click", "--hit", "settings"]);
    session.ok(&["wait", "--text", "[ ] Color blind mode"]);
    session.ok(&["click", "--hit", "color-blind"]);
    session.ok(&["wait", "--changed", "--rows", "0-10"]);
    session.ok(&["wait", "--text", "[x] Color blind mode"]);
    session.ok(&["wait", "--state", "settings.color_blind_mode == true"]);
    session.ok(&["press", "esc"]);

    // Ctrl+V pastes the fixture image through the clipboard stand-in.
    session.ok(&["fixture", "clipboard-image"]);
    session.ok(&["click", "--hit", "composer"]);
    session.ok(&["press", "ctrl+v"]);
    let read = session.ok(&["wait", "--effect", "clipboard_image"]);
    assert_eq!(read["effect"]["served"], true, "{read}");
    session.ok(&["wait", "--text", "[paste-", "--timeout", "5s"]);
    let (code, missing) = tui(
        &session.0,
        &["wait", "--effect", "open", "--timeout", "300ms"],
    );
    assert_eq!(code, 1, "{missing}");

    // A style wait reads a named hit's cells: the settings button is not drawn inverse.
    session.ok(&[
        "wait",
        "--style",
        "--hit",
        "settings",
        "--no-attrs",
        "inverse",
    ]);

    let shot = session.ok(&["screenshot"]);
    let path = shot["path"].as_str().unwrap();
    let bytes = std::fs::read(path).unwrap();
    assert!(bytes.starts_with(b"\x89PNG"), "{path} is a PNG");
}

#[test]
fn a_fixture_file_name_leaves_the_selected_session_alone() {
    let session = Session::start("fixture");
    let output = bus(&session.0)
        .args(["--dev", "tui", "fixture", "file", "--file", "notes.txt"])
        .env("BUS_TUI_SESSION", &session.0)
        .output()
        .expect("create a named fixture in the selected scratch session");
    let (code, fixture) = parse(output);
    assert_eq!(code, 0, "named fixture failed: {fixture}");
    assert_eq!(
        PathBuf::from(fixture["path"].as_str().unwrap())
            .file_name()
            .unwrap(),
        "notes.txt"
    );
}

/// The first `perf/client.jsonl` window under `dir` that has every named metric.
fn client_perf_window(dir: &Path, metrics: &[&str]) -> Option<Value> {
    fn find(dir: &Path) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find(&path) {
                    return Some(found);
                }
            } else if path.ends_with("perf/client.jsonl") {
                return Some(path);
            }
        }
        None
    }
    let text = std::fs::read_to_string(find(dir)?).ok()?;
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| {
            record["kind"] == "window"
                && metrics
                    .iter()
                    .all(|metric| record["timers"].get(*metric).is_some())
        })
}

#[test]
fn the_client_records_ui_loop_and_coordinator_perf_windows() {
    let session = Session::start("perf");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    session.ok(&["press", "ctrl+shift+r"]);
    session.ok(&["wait", "--text", "Room name"]);
    session.ok(&["press", "escape"]);
    let metrics = [
        "client.ui_loop",
        "client.timer_lag",
        "client.compose",
        "client.present",
        "coordinator.loop",
        "coordinator.snapshot",
        "coordinator.tick",
    ];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut window = None;
    while window.is_none() && std::time::Instant::now() < deadline {
        window = client_perf_window(&root(&session.0), &metrics);
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let window = window.unwrap_or_else(|| {
        panic!(
            "no client perf window with {metrics:?} under {}",
            root(&session.0).display()
        )
    });
    assert_eq!(window["role"], "client");
    assert!(window["rss_mb"].as_f64().unwrap() > 0.0, "{window}");
}

/// Where the overlay label shows, if it does.
fn overlay_matches(session: &Session) -> Vec<Value> {
    let found = session.ok(&["find", "CPU [0-9n].* MEM .* FPS", "--regex"]);
    found["matches"].as_array().cloned().unwrap_or_default()
}

#[test]
fn the_dev_performance_overlay_turns_on_updates_and_turns_off() {
    let session = Session::start("overlay");
    session.ok(&["wait", "--text", "# bus", "--timeout", "10s"]);
    session.ok(&["click", "--text", "Settings"]);
    session.ok(&["scroll", "--text", "[x] You", "--down", "10"]);
    session.ok(&["wait", "--text", "[ ] Performance overlay"]);
    assert!(overlay_matches(&session).is_empty(), "off by default");

    session.ok(&["click", "--text", "Performance overlay"]);
    session.ok(&["wait", "--text", "[x] Performance overlay"]);
    session.ok(&["wait", "--text", "MB FPS", "--timeout", "5s"]);
    let shown = overlay_matches(&session);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0]["row"].as_u64().unwrap() <= 1, "top row: {shown:?}");
    // The label refreshes every second; FPS is measured after the first full second.
    session.ok(&["wait", "--text", "FPS [0-9]+", "--regex", "--timeout", "5s"]);
    let first = shown[0]["text"].clone();
    session.ok(&["wait", "--changed", "--rows", "0-1", "--timeout", "5s"]);
    assert_eq!(
        overlay_matches(&session).len(),
        1,
        "still shown after {first}"
    );

    session.ok(&["click", "--text", "Performance overlay"]);
    session.ok(&["wait", "--text", "[ ] Performance overlay"]);
    session.ok(&["wait", "--text", "MB FPS", "--gone", "--timeout", "5s"]);
}

#[test]
fn scriptable_fake_replies_to_a_room_message_without_provider_configuration() {
    let session = Session::start("scripted");
    let work = session_dir(&session.0).join("work");
    let script = work.join("fake-script.json");
    std::fs::write(
        &script,
        r#"{"version":1,"turns":[{"contains":"CASE_REPLY_7","actions":[{"type":"reply","text":"REPLY_7"}]}]}"#,
    )
    .unwrap();
    let args = format!("--script {}", script.display());
    let (code, added) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "scripted-worker",
            "--provider",
            "fake",
            "--pwd",
            work.to_str().unwrap(),
            "--args",
            &args,
        ],
    ));
    assert_eq!(code, 0, "{added}");
    let (code, sent) = parse(scratch_bus(
        &session,
        &[
            "send",
            "--room",
            "bus",
            "--to",
            "scripted-worker",
            "--text",
            "CASE_REPLY_7",
        ],
    ));
    assert_eq!(code, 0, "{sent}");
    let id = sent["result"]["message_id"].to_string();
    let result = session.ok(&["wait", "--message", &id, "--timeout", "30s"]);
    let request = &result["message"]["result"]["requests"][0];
    assert_eq!(request["stage"], "replied", "{result}");
    assert_eq!(request["reply"]["text"], "REPLY_7", "{result}");
    assert!(!work.join(".claude").exists());
    assert!(!work.join(".codex").exists());
    assert!(!work.join(".cursor").exists());
}

#[test]
fn scriptable_fake_waits_for_dialog_choice_before_replying() {
    let session = Session::start("f-dialog");
    let work = session_dir(&session.0).join("work");
    let script = work.join("dialog.json");
    std::fs::write(&script, r#"{"version":1,"turns":[{"contains":"CASE_DIALOG_7","actions":[{"type":"dialog","title":"Choose a path","options":["A","B"]},{"type":"reply","text":"selected {{choice}}"}]}]}"#).unwrap();
    let args = format!("--script {}", script.display());
    let (code, added) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "dialog-worker",
            "--provider",
            "fake",
            "--pwd",
            work.to_str().unwrap(),
            "--args",
            &args,
        ],
    ));
    assert_eq!(code, 0, "{added}");
    let (code, sent) = parse(scratch_bus(
        &session,
        &[
            "send",
            "--room",
            "bus",
            "--to",
            "dialog-worker",
            "--text",
            "CASE_DIALOG_7",
        ],
    ));
    assert_eq!(code, 0, "{sent}");
    let id = sent["result"]["message_id"].to_string();
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == blocked",
        "--timeout",
        "20s",
    ]);
    let (code, status) = parse(scratch_bus(&session, &["message", "status", &id]));
    assert_eq!(code, 0, "{status}");
    let request = &status["result"]["requests"][0];
    assert_eq!(request["start_bound"], true, "{status}");
    assert!(
        request["turn_id"]
            .as_str()
            .is_some_and(|turn| !turn.is_empty()),
        "{status}"
    );
    let (code, observed) = parse(scratch_bus(&session, &["agent", "dialog", "dialog-worker"]));
    assert_eq!(code, 0, "{observed}");
    let dialog = &observed["result"];
    assert!(
        dialog["dialog"].to_string().contains("Choose a path"),
        "{dialog}"
    );
    let fingerprint = dialog["fingerprint"].as_str().unwrap();
    let (code, selected) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "choose",
            "dialog-worker",
            "--option",
            "2",
            "--fingerprint",
            fingerprint,
        ],
    ));
    assert_eq!(code, 0, "{selected}");
    let result = session.ok(&["wait", "--message", &id, "--timeout", "30s"]);
    let request = &result["message"]["result"]["requests"][0];
    assert_eq!(request["stage"], "replied", "{result}");
    assert_eq!(request["reply"]["text"], "selected B", "{result}");
}

#[test]
fn scriptable_fake_sends_before_its_final_reply() {
    let session = Session::start("f-send");
    let work = session_dir(&session.0).join("work");
    let script = work.join("send.json");
    std::fs::write(&script, r#"{"version":1,"turns":[{"contains":"CASE_SEND_7","actions":[{"type":"send","room":"bus","to":"you","text":"SCRIPTED_NOTICE_7"},{"type":"reply","text":"SENT_7"}]}]}"#).unwrap();
    let args = format!("--script {}", script.display());
    let (code, added) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "sender-worker",
            "--provider",
            "fake",
            "--pwd",
            work.to_str().unwrap(),
            "--args",
            &args,
        ],
    ));
    assert_eq!(code, 0, "{added}");
    let (code, sent) = parse(scratch_bus(
        &session,
        &[
            "send",
            "--room",
            "bus",
            "--to",
            "sender-worker",
            "--text",
            "CASE_SEND_7",
        ],
    ));
    assert_eq!(code, 0, "{sent}");
    let id = sent["result"]["message_id"].to_string();
    let result = session.ok(&["wait", "--message", &id, "--timeout", "30s"]);
    assert!(result.to_string().contains("SENT_7"), "{result}");
    let (code, history) = parse(scratch_bus(
        &session,
        &["history", "--room", "bus", "--limit", "100"],
    ));
    assert_eq!(code, 0, "{history}");
    assert!(
        history.to_string().contains("SCRIPTED_NOTICE_7"),
        "{history}"
    );
}

#[test]
fn scriptable_fake_crash_leaves_no_final_reply_and_records_exit() {
    let session = Session::start("f-crash");
    let work = session_dir(&session.0).join("work");
    let script = work.join("crash.json");
    std::fs::write(&script, r#"{"version":1,"turns":[{"contains":"CASE_CRASH_7","actions":[{"type":"crash","code":42}]}]}"#).unwrap();
    let args = format!("--script {}", script.display());
    let (code, added) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "crash-worker",
            "--provider",
            "fake",
            "--pwd",
            work.to_str().unwrap(),
            "--args",
            &args,
        ],
    ));
    assert_eq!(code, 0, "{added}");
    let (code, sent) = parse(scratch_bus(
        &session,
        &[
            "send",
            "--room",
            "bus",
            "--to",
            "crash-worker",
            "--text",
            "CASE_CRASH_7",
        ],
    ));
    assert_eq!(code, 0, "{sent}");
    let id = sent["result"]["message_id"].to_string();
    session.ok(&[
        "wait",
        "--state",
        "agents.*.exit_snapshot.terminal_closed == true",
        "--timeout",
        "30s",
    ]);
    let (code, status) = parse(scratch_bus(&session, &["message", "status", &id]));
    assert_eq!(code, 0, "{status}");
    assert!(
        status["result"]["requests"][0]["reply"].is_null(),
        "{status}"
    );
    assert!(
        status["result"]["requests"][0]["exit_snapshot"].is_object(),
        "{status}"
    );
}

#[test]
fn scriptable_fake_resumes_without_reusing_prior_turns() {
    let session = Session::start("f-resume");
    let work = session_dir(&session.0).join("work");
    let script = work.join("resume.json");
    std::fs::write(&script, r#"{"version":1,"turns":[{"contains":"CASE_RESUME_7","actions":[{"type":"reply","text":"ONCE_7"}]}]}"#).unwrap();
    let args = format!("--script {}", script.display());
    let (code, added) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "resume-worker",
            "--provider",
            "fake",
            "--pwd",
            work.to_str().unwrap(),
            "--args",
            &args,
        ],
    ));
    assert_eq!(code, 0, "{added}");
    let send = |text: &str| {
        let (code, sent) = parse(scratch_bus(
            &session,
            &[
                "send",
                "--room",
                "bus",
                "--to",
                "resume-worker",
                "--text",
                text,
            ],
        ));
        assert_eq!(code, 0, "{sent}");
        let id = sent["result"]["message_id"].to_string();
        session.ok(&["wait", "--message", &id, "--timeout", "30s"])
    };
    assert!(send("CASE_RESUME_7").to_string().contains("ONCE_7"));
    let before = bus_state(&session)["agents"][0]["runtime_identity"]["session_id"].clone();
    session.ok(&["restart"]);
    session.ok(&[
        "wait",
        "--state",
        "agents.0.status == idle",
        "--timeout",
        "20s",
    ]);
    assert_eq!(
        bus_state(&session)["agents"][0]["runtime_identity"]["session_id"],
        before
    );
    assert!(send("after restart\nfake agent working")
        .to_string()
        .contains("fake reply:"));
    let repeated = send("CASE_RESUME_7");
    assert!(repeated.to_string().contains("fake reply:"), "{repeated}");
    assert!(!repeated.to_string().contains("ONCE_7"), "{repeated}");
}
