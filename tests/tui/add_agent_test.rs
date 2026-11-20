//! Drive Add through the scratch TUI, including native shell startup.
use super::{bus_state, parse, run_dir, scratch_bus, session_dir, session_name, tui_raw, Session};
use std::{fs, path::PathBuf};

const FORM: &str = "Additional launch args (optional)";

fn form(session: &Session, name: &str, provider: &str, pwd: &str) {
    session.ok(&["press", "Ctrl+n"]);
    if !name.is_empty() {
        session.ok(&["type", name]);
    }
    session.ok(&["press", "Tab"]);
    session.ok(&["click", "--hit", &format!("provider:{provider}")]);
    session.ok(&["press", "Ctrl+a"]);
    session.ok(&["press", "Ctrl+k"]);
    session.ok(&["type", pwd]);
}

fn home(session: &Session) -> PathBuf {
    session_dir(&session.0).join("home")
}

fn screen(session: &Session) -> String {
    session.ok(&["snapshot"])["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

fn no_agent(session: &Session, name: &str) {
    assert!(!bus_state(session)["agents"]
        .as_array()
        .unwrap()
        .iter()
        .any(|agent| agent["name"] == name));
}

#[test]
fn duplicate_across_rooms_and_empty_name_stay_inline() {
    let session = Session::start("add-dup");
    let pwd = home(&session);
    let (code, reply) = parse(scratch_bus(&session, &["room", "create", "elsewhere"]));
    assert_eq!(code, 0, "{reply}");
    let (code, reply) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "elsewhere",
            "--name",
            "taken",
            "--provider",
            "claude",
            "--pwd",
            pwd.to_str().unwrap(),
        ],
    ));
    assert_eq!(code, 0, "{reply}");
    form(&session, "taken", "claude", "~/");
    session.ok(&["press", "Enter"]);
    session.ok(&[
        "wait",
        "--text",
        "is already used by agent",
        "--timeout",
        "2s",
    ]);
    assert!(screen(&session).contains(FORM));
    session.ok(&["press", "Esc"]);
    form(&session, "", "claude", "~/");
    session.ok(&["press", "Enter"]);
    session.ok(&["wait", "--text", "Name is required.", "--timeout", "2s"]);
    assert!(screen(&session).contains(FORM));
    assert_eq!(bus_state(&session)["agents"].as_array().unwrap().len(), 1);
}

#[test]
fn pwd_edits_completes_with_mouse_and_expands_tilde_on_add() {
    let session = Session::start("add-pwd");
    let pwd = home(&session).join("work/bus");
    let neighbor = home(&session).join("work/bus-other");
    fs::create_dir_all(&pwd).unwrap();
    fs::create_dir_all(&neighbor).unwrap();
    form(&session, "tilde-worker", "claude", "~/work/buzs");
    session.ok(&["press", "Left"]);
    session.ok(&["press", "Backspace"]);
    let path = session.ok(&["find", "~/work/bus"]);
    let snapshot = session.ok(&["snapshot"]);
    let hit = &path["matches"][0];
    assert_eq!(snapshot["cursor"]["visible"], true, "{snapshot}");
    assert_eq!(snapshot["cursor"]["row"], hit["row"], "{path}");
    assert_eq!(
        snapshot["cursor"]["col"].as_u64().unwrap(),
        hit["col"].as_u64().unwrap() + 9
    );
    session.ok(&["wait", "--text", &format!("{}/", neighbor.display())]);
    session.ok(&["press", "Down"]);
    assert!(screen(&session).contains("~/work/bus"));
    session.ok(&["click", "--hit", "suggestion:1"]);
    let snapshot = screen(&session);
    assert_eq!(
        snapshot.matches(neighbor.to_str().unwrap()).count(),
        1,
        "{snapshot}"
    );
    assert!(!snapshot.contains("~/work/bus"), "{snapshot}");
    session.ok(&["press", "Ctrl+a"]);
    session.ok(&["press", "Ctrl+k"]);
    session.ok(&["type", "~/work/bus"]);
    session.ok(&["press", "Enter"]);
    assert!(!screen(&session).contains(FORM));
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "15s",
    ]);
    assert_eq!(
        bus_state(&session)["agents"][0]["cwd"],
        pwd.canonicalize().unwrap().to_str().unwrap()
    );
}

#[test]
fn invalid_pwd_closes_and_reports_failure_in_the_room() {
    let session = Session::start("add-bad");
    form(&session, "bad-pwd", "claude", "~/missing");
    session.ok(&["press", "Enter"]);
    assert!(!screen(&session).contains(FORM));
    session.ok(&["wait", "--text", "Agent PWD", "--timeout", "2s"]);
    assert!(screen(&session).contains("bad-pwd"));
    no_agent(&session, "bad-pwd");
}

#[test]
fn unavailable_provider_closes_and_reports_failure_in_the_room() {
    let session = Session::hold(session_name("add-miss"));
    let (code, status) = parse(tui_raw(
        &session.0,
        &[
            "start",
            "--size",
            "140x40",
            "--run-dir",
            run_dir().to_str().unwrap(),
        ],
        &[("PATH", "/usr/bin:/bin")],
    ));
    assert_eq!(code, 0, "{status}");
    fs::remove_file(session_dir(&session.0).join("bin/claude")).unwrap();
    form(&session, "missing-provider", "claude", "~/");
    session.ok(&["press", "Enter"]);
    assert!(!screen(&session).contains(FORM));
    session.ok(&[
        "wait",
        "--text",
        "provider binary 'claude' is missing",
        "--timeout",
        "3s",
    ]);
    no_agent(&session, "missing-provider");
}

#[test]
fn slow_add_closes_immediately_and_keeps_the_room_editable() {
    let session = Session::start("add-slow");
    fs::write(home(&session).join(".bash_profile"), "/bin/sleep 9\n").unwrap();
    form(&session, "slow-worker", "claude", "~/");
    session.ok(&["press", "Enter"]);
    let immediate = screen(&session);
    session.ok(&["type", "draft during startup"]);
    let editing = screen(&session);
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "20s",
    ]);
    assert!(!immediate.contains(FORM), "{immediate}");
    assert!(immediate.contains("slow-worker"), "{immediate}");
    assert!(immediate.contains("Launching"), "{immediate}");
    assert!(editing.contains("draft during startup"), "{editing}");
    assert!(screen(&session).contains("draft during startup"));
}

#[test]
fn native_startup_deadline_is_visible_in_the_room() {
    let session = Session::start("add-time");
    fs::write(home(&session).join(".bash_profile"), "/bin/sleep 36\n").unwrap();
    form(&session, "deadline-worker", "claude", "~/");
    session.ok(&["press", "Enter"]);
    let immediate = screen(&session);
    session.ok(&["wait", "--text", "after 30 seconds", "--timeout", "38s"]);
    assert!(!immediate.contains(FORM), "{immediate}");
    let settled = screen(&session);
    assert!(settled.contains("deadline-worker"), "{settled}");
    assert!(!settled.contains(FORM), "{settled}");
}

#[test]
fn escape_cancels_without_launching() {
    let session = Session::start("add-esc");
    form(&session, "cancelled-worker", "claude", "~/");
    session.ok(&["press", "Esc"]);
    assert!(!screen(&session).contains(FORM));
    no_agent(&session, "cancelled-worker");
}

#[test]
fn project_hook_consent_remains_required_after_the_form_closes() {
    let session = Session::start("add-hook");
    let project = home(&session).join("project");
    fs::create_dir(&project).unwrap();
    form(&session, "codex-worker", "codex", "~/project");
    session.ok(&["press", "Enter"]);
    session.ok(&["wait", "--text", "Bus will add a few hooks"]);
    no_agent(&session, "codex-worker");
    assert!(!project.join(".codex/hooks.json").exists());
    session.ok(&["click", "--hit", "add"]);
    assert!(!screen(&session).contains("Bus will add a few hooks"));
    session.ok(&[
        "wait",
        "--state",
        "agents.*.status == idle",
        "--timeout",
        "15s",
    ]);
    assert!(project.join(".codex/hooks.json").is_file());
    assert!(screen(&session).contains("codex-worker"));
}
