//! `bus --dev tui` against the built binary: a scratch Bus driven only through
//! the driver CLI, and the guards that keep it away from every other Bus.
// Only the dev variant has the driver; the gate runs this target in its
// dev-tools lane, and an explicit `--test '*'` must still build without it.
// Sessions start only on macOS, where the effect recorder covers Bus.
#![cfg(all(target_os = "macos", feature = "dev-tools"))]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

impl Session {
    fn start(tag: &str) -> Self {
        let name = session_name(tag);
        let runs = run_dir();
        let (code, status) = tui(
            &name,
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
        Self(name)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, value) = tui(&self.0, args);
        assert_eq!(code, 0, "{args:?} failed: {value}");
        value
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = tui(&self.0, &["stop"]);
        // A stopped session leaves no root behind.
        let _ = std::fs::remove_file(root(&self.0).join(".start.lock"));
        let _ = std::fs::remove_dir(root(&self.0));
    }
}

fn bus_state(session: &Session) -> Value {
    let output = scratch_bus(session, &["state"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<Value>(stdout.trim()).unwrap()["result"].clone()
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
    let clicked = session.ok(&["click", "--text", "# bus", "--rows", "3-8"]);
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
    let session = Session::start("codex");
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
            "codex",
            "--pwd",
            work.to_str().unwrap(),
            "--consent-hooks",
        ],
    );
    assert!(added.status.success(), "{added:?}");
    assert!(work.join(".codex/hooks.json").is_file());
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

    // Bus types each prompt into the fake's Codex composer; its hooks reply.
    for text in ["hello codex", "hello again"] {
        let id = send(text);
        let settled = session.ok(&["wait", "--message", &id, "--timeout", "30s"]);
        assert!(
            settled.to_string().contains(&format!("fake reply: {text}")),
            "the fake Codex should answer {text:?}: {settled}"
        );
    }

    // A message sent while the fake Codex works is typed into its running
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
    let caller = std::env::temp_dir().join(format!("bus-tui-it-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&caller).unwrap();
    let caller = caller.canonicalize().unwrap();
    let started = bus(&name)
        .current_dir(&caller)
        .args(["--dev", "tui", "start", "--name", &name, "--size", "80x24"])
        .args(["--run-dir", "runs"])
        .output()
        .unwrap();
    let session = Session(name);
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
    let session = Session(name);
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
    let session = Session(name);
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
