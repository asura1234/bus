use super::*;
use crate::agents::AgentState;

#[tokio::test]
async fn native_kimi_start_uses_its_home_for_command_detection() {
    let dir = crate::utils::test_temp::unique_temp_path("native-kimi-wire");
    let home = dir.join("kimi-home");
    let session = home.join("sessions/wd/session_1");
    crate::platform::fs::private_dir(&session.join("agents/main")).unwrap();
    let index = serde_json::json!({"sessionId":"session_1","sessionDir":session});
    std::fs::write(home.join("session_index.jsonl"), format!("{index}\n")).unwrap();
    let wire = session.join("agents/main/wire.jsonl");
    let prompt = "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":1,\"promptId\":\"p1\",\"input\":[{\"type\":\"text\",\"text\":\"work\"}]}\n";
    std::fs::write(&wire, prompt).unwrap();
    let (runtime, mut writes) = TerminalRuntime::test_with_channel(80, 24);
    runtime.test_process_pty_bytes(b"\x1b]133;A\x07ready> ");
    let (mut app, target) = app_with_prompt_runtime(runtime);
    app.start_agent(crate::protocol::api::schema::AgentStartParams {
        name: "kimi-wire".into(),
        kind: "kimi".into(),
        pane_id: target,
        args: vec![],
        executable: Some(dir.join("bin/kimi").display().to_string()),
        grok_home: None,
        kimi_home: Some(home.display().to_string()),
        timeout_ms: None,
    })
    .unwrap_or_else(|err| panic!("{:?}", app.agent_start_error_body(err)));
    let input = writes.recv().await.unwrap();
    assert!(String::from_utf8_lossy(&input).contains(&format!("KIMI_CODE_HOME={}", home.display())));
    let runtime = prompt_runtime(&app);
    runtime.test_process_pty_bytes("\x1b[2J\x1b[HRunning a command\r\n╭────────────────────────╮\r\n│ >                      │\r\n╰────────────────────────╯\r\nK3.1 Pro thinking: high\r\ncontext: 2% (17.9k/1M)\r\n".as_bytes());
    assert_eq!(
        runtime.test_kimi_command_detection().unwrap().state,
        AgentState::Working,
        "native Kimi detector must follow its launch home's wire"
    );
    std::fs::write(&wire, format!("{prompt}{{\"type\":\"agent.turn.ended\",\"turnId\":1,\"outcome\":\"done\"}}\n{{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":1,\"reason\":\"completed\"}}\n{{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"p1\",\"reason\":\"completed\"}}\n")).unwrap();
    assert_eq!(
        runtime.test_kimi_command_detection().unwrap().state,
        AgentState::Idle,
        "an unchanged command frame must settle with its native wire"
    );
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
