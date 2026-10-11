//! `bus --dev tui` against the built binary: a scratch Bus driven only through
//! the driver CLI, and the guards that keep it away from every other Bus.
#![cfg(unix)]

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

fn tui(name: &str, args: &[&str]) -> (i32, Value) {
    let output = tui_raw(name, args, &[]);
    parse(output)
}

fn tui_raw(name: &str, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bus"));
    command
        .args(["--dev", "tui"])
        .args(args)
        .args(["--name", name]);
    // cargo builds the driver into target/debug; allow it for this suite only.
    command.env("BUS_TUI_ALLOW_TARGET_DEBUG", "1");
    for key in [
        "BUS_DATA_DIR",
        "BUS_SESSION_ID",
        "HERDR_SOCKET_PATH",
        "BUS_TUI_SESSION",
    ] {
        command.env_remove(key);
    }
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
    }
}

fn bus_state(session: &Session) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .args(["--dev", "tui", "bus", "--name", &session.0, "state"])
        .env("BUS_TUI_ALLOW_TARGET_DEBUG", "1")
        .output()
        .unwrap();
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
    let work = format!("/tmp/bus-tui/{}/work", session.0);
    let status = Command::new(env!("CARGO_BIN_EXE_bus"))
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
            &work,
        ])
        .env("BUS_TUI_ALLOW_TARGET_DEBUG", "1")
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

    let resized = session.ok(&["resize", "100x30"]);
    assert_eq!(resized["size"][0], 100);
    let snapshot = session.ok(&["snapshot"]);
    assert_eq!(snapshot["size"][1], 30);

    let (code, stopped) = tui(&session.0, &["stop"]);
    assert_eq!(code, 0, "{stopped}");
    let run = PathBuf::from(stopped["run_dir"].as_str().unwrap());
    assert!(run.join("trace.ndjson").is_file());
    assert!(run.join("pty.cast").is_file());
    assert!(!Path::new(&format!("/tmp/bus-tui/{}", session.0)).exists());
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
    let target = format!("/tmp/bus-tui/{other}/data");
    let (code, refused) = parse(tui_raw(&other, &["start"], &[("BUS_DATA_DIR", &target)]));
    assert_eq!(code, 1, "{refused}");
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("BUS_DATA_DIR"));

    // A directory a live Bus serves is refused, even at the driver's own path.
    let foreign = session_name("live");
    let data = PathBuf::from(format!("/tmp/bus-tui/{foreign}/data"));
    std::fs::create_dir_all(&data).unwrap();
    let socket = data.join("control.sock");
    let _live = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let (code, refused) = tui(&foreign, &["start"]);
    assert_eq!(code, 1, "{refused}");
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("live Bus"));
    let _ = std::fs::remove_dir_all(format!("/tmp/bus-tui/{foreign}"));

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
