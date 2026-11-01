use super::*;

fn scripted_message(session: &Session, actions: Value) -> String {
    let work = session_dir(&session.0).join("work");
    let script = work.join("wait-outcome.json");
    let scenario = serde_json::json!({
        "version": 1,
        "turns": [{"contains": "WAIT_OUTCOME_CASE", "actions": actions}]
    });
    std::fs::write(&script, serde_json::to_vec(&scenario).unwrap()).unwrap();
    let args = format!("--script {}", script.display());
    let (code, added) = parse(scratch_bus(
        session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "wait-worker",
            "--provider",
            "fake",
            "--pwd",
            work.to_str().unwrap(),
            "--args",
            &args,
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
    let (code, sent) = parse(scratch_bus(
        session,
        &[
            "send",
            "--room",
            "bus",
            "--to",
            "wait-worker",
            "--text",
            "WAIT_OUTCOME_CASE",
        ],
    ));
    assert_eq!(code, 0, "{sent}");
    sent["result"]["message_id"].to_string()
}

#[test]
fn bounded_message_wait_reports_a_settled_provider_error() {
    let session = Session::start("wf");
    let work = session_dir(&session.0).join("work");
    let (code, added) = parse(scratch_bus(
        &session,
        &[
            "agent",
            "add",
            "--room",
            "bus",
            "--name",
            "error-worker",
            "--provider",
            "claude",
            "--pwd",
            work.to_str().unwrap(),
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
    let (code, sent) = parse(scratch_bus(
        &session,
        &[
            "send",
            "--room",
            "bus",
            "--to",
            "error-worker",
            "--text",
            "fake:api-error",
        ],
    ));
    assert_eq!(code, 0, "{sent}");
    let message = sent["result"]["message_id"].to_string();
    let (code, result) = tui(
        &session.0,
        &["wait", "--message", &message, "--timeout", "30s"],
    );
    assert_eq!(code, 1, "{result}");
    assert_eq!(
        result["message"]["error"]["code"], "message_provider_error",
        "{result}"
    );
    assert_eq!(result["message"]["result"]["complete"], true, "{result}");
    assert_eq!(result["error"], result["message"]["error"], "{result}");
    assert!(result["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Connection lost mid-response"));
}

#[test]
fn bounded_message_wait_preserves_an_actual_timeout() {
    let session = Session::start("wt");
    let message = scripted_message(
        &session,
        serde_json::json!([{"type": "sleep", "ms": 5000}, {"type": "reply", "text": "LATE"}]),
    );
    let (code, result) = tui(
        &session.0,
        &["wait", "--message", &message, "--timeout", "1s"],
    );
    assert_eq!(code, 1, "{result}");
    assert_eq!(result["error"]["code"], "timeout", "{result}");
    assert_eq!(result["error"], result["message"]["error"], "{result}");
    assert_eq!(result["message"]["result"]["complete"], false, "{result}");
}

#[test]
fn bounded_message_wait_preserves_an_invalid_message_error() {
    let session = Session::start("wi");
    let (code, result) = tui(
        &session.0,
        &["wait", "--message", "999999999", "--timeout", "1s"],
    );
    assert_eq!(code, 1, "{result}");
    assert_eq!(result["error"], result["message"]["error"], "{result}");
    assert_ne!(result["error"]["code"], "timeout", "{result}");
}

#[test]
fn bounded_message_wait_reports_confirmed_bus_exit() {
    let session = Session::start("wd");
    let (code, stopped) = parse(scratch_bus(&session, &["stop"]));
    assert_eq!(code, 0, "{stopped}");
    assert_eq!(stopped["stopped"], true, "{stopped}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let status = session.ok(&["status"]);
        if status["alive"] == false {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{status}");
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let (code, result) = tui(&session.0, &["wait", "--message", "1", "--timeout", "2s"]);
    assert_eq!(code, 3, "{result}");
    assert_eq!(result["error"]["code"], "bus_exited", "{result}");
}
