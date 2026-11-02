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

const BACKGROUND_IDLE: &str =
    include_str!("../../../../tests/fixtures/kimi-background/idle-tip.txt");

struct BackgroundWire {
    home: std::path::PathBuf,
    wire: std::path::PathBuf,
}

impl BackgroundWire {
    fn new() -> Self {
        let home = crate::utils::test_temp::unique_temp_path("kimi-background-wire");
        let session = home.join("sessions/wd/session_1");
        crate::platform::fs::private_dir(&session.join("agents/main")).unwrap();
        let index = serde_json::json!({"sessionId":"session_1","sessionDir":session});
        std::fs::write(home.join("session_index.jsonl"), format!("{index}\n")).unwrap();
        Self {
            home,
            wire: session.join("agents/main/wire.jsonl"),
        }
    }

    fn write(&self, settled: bool, background: bool) {
        let mut rows = vec![
            serde_json::json!({"type":"turn.prompt","agentId":"main","turnId":1,"promptId":"p1","input":[{"type":"text","text":"start a background watch loop"}]}),
        ];
        if settled {
            rows.extend([
                serde_json::json!({"type":"agent.turn.ended","turnId":1,"outcome":"done"}),
                serde_json::json!({"type":"turn.ended","agentId":"main","turnId":1,"reason":"completed"}),
                serde_json::json!({"type":"prompt.completed","agentId":"main","promptId":"p1","reason":"completed"}),
            ]);
        }
        if background {
            rows.push(serde_json::json!({"type":"agent.message.appended","message":{"meta":{"source":"tool"},"message":{"role":"tool","toolCallId":"bash-1","content":[{"type":"text","text":"task_id: bash-1\nstatus: running\nautomatic_notification: true"}]}}}));
        }
        std::fs::write(
            &self.wire,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
}

impl Drop for BackgroundWire {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).unwrap();
    }
}

fn background_runtime(screen: &str, home: Option<&std::path::Path>) -> TerminalRuntime {
    let runtime =
        TerminalRuntime::test_with_screen_bytes(120, 40, screen.replace('\n', "\r\n").as_bytes());
    runtime.bind_kimi_code_home(home.map(std::path::Path::to_path_buf));
    runtime
}

#[tokio::test]
async fn kimi_background_captured_idle_tip_is_idle_after_main_turn_ends() {
    let wire = BackgroundWire::new();
    wire.write(true, true);
    for previous in [AgentState::Working, AgentState::Unknown] {
        let runtime = background_runtime(BACKGROUND_IDLE, Some(&wire.home));
        let result = runtime
            .test_scan_detection_context(AgentKind::Kimi, previous)
            .await;
        assert_eq!(
            result.0,
            AgentState::Idle,
            "captured idle prompt; previous={previous:?}"
        );
    }
}

#[tokio::test]
async fn kimi_background_task_count_does_not_block_an_idle_composer() {
    let wire = BackgroundWire::new();
    wire.write(true, true);
    for count in ["[1 task running]", "[2 tasks running]"] {
        let screen = BACKGROUND_IDLE
            .replace("  🌒 · Tip: ! to run a shell command\n", "")
            .replace("Ask When Needed", &format!("{count} Ask When Needed"));
        for home in [Some(wire.home.as_path()), None] {
            let runtime = background_runtime(&screen, home);
            let result = runtime.test_kimi_command_detection().unwrap();
            assert_eq!(result.state, AgentState::Idle, "{count}, wire={home:?}");
            assert!(result.visible_idle);
        }
    }
}

#[tokio::test]
async fn kimi_background_start_end_restart_preserves_foreground_working() {
    let wire = BackgroundWire::new();
    for background in [false, true, false] {
        wire.write(true, background);
        // A fresh runtime covers restart with a settled native main turn.
        let screen = if background {
            BACKGROUND_IDLE.replace("Ask When Needed", "[1 task running] Ask When Needed")
        } else {
            BACKGROUND_IDLE.to_owned()
        };
        let runtime = background_runtime(&screen, Some(&wire.home));
        assert_eq!(
            runtime.test_kimi_command_detection().unwrap().state,
            AgentState::Idle,
            "background={background}"
        );
        wire.write(false, background);
        assert_eq!(
            runtime.test_kimi_command_detection().unwrap().state,
            AgentState::Working,
            "an open main turn with a tip remains busy"
        );
    }
}

#[tokio::test]
async fn kimi_background_real_busy_markers_stay_working_with_settled_wire() {
    let wire = BackgroundWire::new();
    wire.write(true, true);
    let without_tip =
        BACKGROUND_IDLE.replace("  🌒 · Tip: ! to run a shell command", "[1 task running]");
    for marker in ["🌒", "⠋ Thinking…", "⠋ Working…", "⠋ Using Bash"] {
        let screen = format!("{marker}\n{without_tip}");
        let runtime = background_runtime(&screen, Some(&wire.home));
        assert_eq!(
            runtime.test_kimi_command_detection().unwrap().state,
            AgentState::Working,
            "{marker}"
        );
    }
    let runtime = background_runtime(BACKGROUND_IDLE, None);
    assert_eq!(
        runtime.test_kimi_command_detection().unwrap().state,
        AgentState::Working,
        "a tip without native evidence stays conservative"
    );
}

#[test]
fn kimi_background_main_turn_settles_after_internal_tool_iterations() {
    let wire = BackgroundWire::new();
    wire.write(true, true);
    let text = std::fs::read_to_string(&wire.wire).unwrap();
    let rows = text
        .lines()
        .map(|line| {
            let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
            if row["type"] == "agent.turn.ended" {
                // Kimi's internal LLM/tool iteration counter is independent of the
                // outer turn.prompt/turn.ended counter (as in committed_outcome).
                row["turnId"] = serde_json::json!(9);
            }
            format!("{row}\n")
        })
        .collect::<String>();
    std::fs::write(&wire.wire, rows).unwrap();
    let turn = crate::agents::providers::kimi::wire::refresh_main_turn(
        &wire.home,
        &mut Default::default(),
    )
    .unwrap();
    assert_eq!(
        turn,
        Some(crate::agents::providers::kimi::wire::MainTurn::Settled)
    );
}

#[tokio::test]
async fn kimi_background_captured_autonomous_foreground_turn_stays_working() {
    let screen = include_str!("../../../../tests/fixtures/kimi-background/foreground-queued.txt");
    let events: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/kimi-background/autonomous-turn-events.json"
    ))
    .unwrap();
    let wire = BackgroundWire::new();
    std::fs::write(
        &wire.wire,
        events
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let runtime = background_runtime(screen, Some(&wire.home));
    assert_eq!(
        runtime.test_kimi_command_detection().unwrap().state,
        AgentState::Working,
        "the captured autonomous turn without promptId must supersede the old settled Bus turn"
    );
    let turn = crate::agents::providers::kimi::wire::refresh_main_turn(
        &wire.home,
        &mut Default::default(),
    )
    .unwrap();
    assert_eq!(
        turn,
        Some(crate::agents::providers::kimi::wire::MainTurn::Open)
    );
}

#[tokio::test]
async fn kimi_background_autonomous_turn_settles_without_prompt_completed() {
    let wire = BackgroundWire::new();
    let events = [
        serde_json::json!({"type":"turn.prompt","agentId":"main","turnId":3}),
        serde_json::json!({"type":"agent.turn.ended","turnId":4,"outcome":"done"}),
        serde_json::json!({"type":"turn.ended","agentId":"main","turnId":3,"reason":"completed"}),
    ];
    std::fs::write(
        &wire.wire,
        events
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let turn = crate::agents::providers::kimi::wire::refresh_main_turn(
        &wire.home,
        &mut Default::default(),
    )
    .unwrap();
    assert_eq!(
        turn,
        Some(crate::agents::providers::kimi::wire::MainTurn::Settled)
    );
    let runtime = background_runtime(BACKGROUND_IDLE, Some(&wire.home));
    assert_eq!(
        runtime.test_kimi_command_detection().unwrap().state,
        AgentState::Idle
    );
}

#[tokio::test]
async fn kimi_open_turn_with_task_count_and_empty_composer_stays_working() {
    let wire = BackgroundWire::new();
    wire.write(false, true);
    let screen = BACKGROUND_IDLE
        .replace("  🌒 · Tip: ! to run a shell command\n", "")
        .replace("Ask When Needed", "[1 task running] Ask When Needed");
    let runtime = background_runtime(&screen, Some(&wire.home));
    assert_eq!(
        runtime.test_kimi_command_detection().unwrap().state,
        AgentState::Working,
        "an open native main turn must outrank background task inventory"
    );
}

#[tokio::test]
async fn kimi_open_turn_without_background_task_stays_working() {
    let wire = BackgroundWire::new();
    wire.write(false, false);
    let screen = BACKGROUND_IDLE.replace("  🌒 · Tip: ! to run a shell command\n", "");
    let runtime = background_runtime(&screen, Some(&wire.home));
    let result = runtime.test_kimi_command_detection().unwrap();
    assert_eq!(result.state, AgentState::Working);
    assert!(!result.visible_idle);
    assert_eq!(
        runtime
            .test_scan_detection_context(AgentKind::Kimi, AgentState::Idle)
            .await
            .0,
        AgentState::Working,
        "the published state must retain the open foreground turn"
    );
}

#[tokio::test]
async fn kimi_empty_composer_without_open_turn_stays_idle() {
    let wire = BackgroundWire::new();
    let screen = BACKGROUND_IDLE.replace("  🌒 · Tip: ! to run a shell command\n", "");
    for settled in [false, true] {
        if settled {
            wire.write(true, false);
        } else {
            std::fs::write(&wire.wire, "").unwrap();
        }
        let runtime = background_runtime(&screen, Some(&wire.home));
        let result = runtime.test_kimi_command_detection().unwrap();
        assert_eq!(result.state, AgentState::Idle, "settled={settled}");
        assert!(result.visible_idle);
        assert!(!result.visible_working);
        assert_eq!(
            runtime
                .test_scan_detection_context(AgentKind::Kimi, AgentState::Working)
                .await
                .0,
            AgentState::Idle,
            "a genuine idle composer must release the previous Working state"
        );
    }
}

#[tokio::test]
async fn kimi_open_turn_with_native_steer_queue_stays_working() {
    let captured = include_str!("../../../../tests/fixtures/kimi-background/foreground-queued.txt");
    let controls = captured
        .lines()
        .filter(|line| line.contains("❯ [") || line.contains("↑ to edit"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(controls.contains("ctrl-s to steer"));
    let wire = BackgroundWire::new();
    for background in [false, true] {
        wire.write(false, background);
        let screen = BACKGROUND_IDLE.replace("  🌒 · Tip: ! to run a shell command\n", "");
        let screen = if background {
            screen.replace("Ask When Needed", "[1 task running] Ask When Needed")
        } else {
            screen
        };
        let runtime = background_runtime(&format!("{controls}\n{screen}"), Some(&wire.home));
        assert_eq!(
            runtime.test_kimi_command_detection().unwrap().state,
            AgentState::Working,
            "queued native guidance cannot make an open turn Idle; background={background}"
        );
        assert_eq!(
            runtime
                .test_scan_detection_context(AgentKind::Kimi, AgentState::Idle)
                .await
                .0,
            AgentState::Working,
            "the native steer queue must remain behind foreground work"
        );
    }
}
