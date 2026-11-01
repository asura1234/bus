use super::*;
use crate::agents::AgentState;

fn command_tip_runtime(bold: bool) -> TerminalRuntime {
    let header = "● Running a command · $ sleep 45";
    let header = if bold {
        format!("\x1b[1m{header}\x1b[0m")
    } else {
        header.to_owned()
    };
    let screen = format!(
        "{header}\r\n Approved: Running: sleep 45\r\n 🌒 · Tip: /goal for multi-step work\r\n╭────────────────────────╮\r\n│ >                      │\r\n╰────────────────────────╯\r\nK3.1 Pro thinking: high\r\ncontext: 2% (17.9k/1M)\r\n"
    );
    TerminalRuntime::test_with_screen_bytes(100, 24, screen.as_bytes())
}

#[tokio::test]
async fn kimi_command_tip_follows_open_and_settled_native_wire() {
    let home = crate::utils::test_temp::unique_temp_path("kimi-tip-settled-wire");
    let session = home.join("sessions/wd/session_1");
    crate::platform::fs::private_dir(&session.join("agents/main")).unwrap();
    let index = serde_json::json!({"sessionId":"session_1","sessionDir":session});
    std::fs::write(home.join("session_index.jsonl"), format!("{index}\n")).unwrap();
    let wire = session.join("agents/main/wire.jsonl");
    let prompt = "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":1,\"promptId\":\"p1\",\"input\":[{\"type\":\"text\",\"text\":\"work\"}]}\n";
    std::fs::write(&wire, prompt).unwrap();
    let runtime = command_tip_runtime(true);
    runtime.bind_kimi_code_home(Some(home.clone()));
    let live = runtime.test_kimi_command_detection().unwrap();
    std::fs::write(&wire, format!("{prompt}{{\"type\":\"agent.turn.ended\",\"turnId\":1,\"outcome\":\"done\"}}\n{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":1,\"reason\":\"completed\"}}\n{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p1\",\"reason\":\"completed\"}}\n")).unwrap();
    let settled = runtime.test_kimi_command_detection().unwrap();
    std::fs::remove_dir_all(home).unwrap();

    assert_eq!(live.state, AgentState::Working);
    assert!(live.visible_working);
    assert_eq!(
        settled.state,
        AgentState::Idle,
        "a settled command's tip must not restore Working"
    );
    assert!(settled.visible_idle);
}

#[tokio::test]
async fn kimi_command_tip_without_wire_requires_a_bold_header() {
    let live = command_tip_runtime(true)
        .test_kimi_command_detection()
        .unwrap();
    assert_eq!(live.state, AgentState::Working);
    assert!(live.visible_working);

    let settled = command_tip_runtime(false)
        .test_kimi_command_detection()
        .unwrap();
    assert_eq!(
        settled.state,
        AgentState::Idle,
        "a non-bold command reply's tip must not restore Working without wire"
    );
    assert!(settled.visible_idle);
}
