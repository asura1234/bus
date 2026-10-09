use super::*;

#[test]
fn codex_is_refused_as_an_orchestrator_but_not_as_a_worker() {
    let (mut worker, _room, _agent, dir) = fixture();
    worker.state.create_room("second").unwrap();
    // Allowed adds get as far as opening a tab, which tests refuse.
    worker.transport = Box::new(NoTabs);
    let cwd = dir.to_string_lossy().to_string();
    let agents_before = worker.state.agents().count();
    for (id, args) in [("plain", ""), ("resume", "resume --last")] {
        let rejected = call(
            &mut worker,
            id,
            "agent.add",
            json!({"room":"master","name":"codex-orch","provider":"codex","cwd":cwd,"orchestrates":"test","extra_args":args,"consent_project_hooks":true}),
        );
        assert!(!rejected.ok, "{id}: {rejected:?}");
        assert_eq!(
            error_message(&rejected),
            crate::messaging::orchestration::CODEX_ORCHESTRATOR_REFUSED,
            "{id}"
        );
    }
    assert_eq!(worker.state.agents().count(), agents_before);
    // Claude Code and Cursor orchestrators, and Codex workers (resuming too),
    // reach the launch.
    for (id, params) in [
        (
            "claude-orch",
            json!({"room":"master","name":"claude-orch","provider":"claude","cwd":cwd,"orchestrates":"test","consent_project_hooks":true}),
        ),
        (
            "cursor-orch",
            json!({"room":"master","name":"cursor-orch","provider":"cursor","cwd":cwd,"orchestrates":"second","consent_project_hooks":true}),
        ),
        (
            "codex-worker",
            json!({"room":"test","name":"codex-worker","provider":"codex","cwd":cwd,"consent_project_hooks":true}),
        ),
        (
            "codex-resume",
            json!({"room":"test","name":"codex-resume","provider":"codex","cwd":cwd,"extra_args":"resume 01a10f9e-71ac-79e2-81b2-56f26341e7e4","consent_project_hooks":true}),
        ),
    ] {
        let response = call(&mut worker, id, "agent.add", params);
        assert!(
            error_message(&response).contains("no tabs in tests"),
            "{id} reached the launch: {response:?}"
        );
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn orchestrators_are_bound_at_add_one_per_room_and_never_reassigned() {
    let (mut worker, room, _agent, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let first = worker
        .state
        .create_agent(
            master,
            "claude-orch",
            Provider::ClaudeCode,
            dir.clone(),
            None,
        )
        .unwrap();
    worker.state.bind_orchestrator(first, room).unwrap();
    let state = call(&mut worker, "state-orch", "state", json!({}));
    assert_eq!(state.result["rooms"][1]["orchestrator"], json!(first));
    let listed = state.result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["id"] == json!(first))
        .unwrap()
        .clone();
    assert_eq!(listed["orchestrates"], json!(room));

    // Reassignment is gone: the room is fixed in the system prompt at launch.
    let reassign = call(
        &mut worker,
        "orch-move",
        "agent.orchestrate",
        json!({"agent":"claude-orch","room":null}),
    );
    assert_eq!(reassign.error.unwrap().code, "unknown_method");
    assert_eq!(worker.state.agent(first).unwrap().orchestrates, Some(room));

    let agents_before = worker.state.agents().count();
    let cwd = dir.to_string_lossy().to_string();
    for (id, params, expected) in [
        (
            "second",
            json!({"room":"master","name":"cursor-orch","provider":"cursor","cwd":cwd,"orchestrates":"test"}),
            "at most one orchestrator",
        ),
        (
            "roomless",
            json!({"room":"master","name":"cursor-orch","provider":"cursor","cwd":cwd}),
            "orchestrates exactly one work room",
        ),
    ] {
        let rejected = call(&mut worker, id, "agent.add", params);
        assert!(!rejected.ok, "{id}: {rejected:?}");
        assert!(
            error_message(&rejected).contains(expected),
            "{id}: {rejected:?}"
        );
    }
    assert_eq!(worker.state.agents().count(), agents_before);

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_agent_add_rejects_an_orchestrator_outside_master_before_launching() {
    let (mut worker, _room, _agent, dir) = fixture();
    let agents_before = worker.state.agents().count();
    let rejected = call(
        &mut worker,
        "add-orch",
        "agent.add",
        json!({
            "room":"test","name":"orch","provider":"cursor",
            "cwd": dir.to_string_lossy(), "orchestrates":"test"
        }),
    );
    assert!(!rejected.ok);
    assert!(
        error_message(&rejected).contains("not in the MASTER room"),
        "{rejected:?}"
    );
    assert_eq!(worker.state.agents().count(), agents_before);

    let unknown = call(
        &mut worker,
        "add-orch-unknown",
        "agent.add",
        json!({
            "room":"master","name":"orch","provider":"codex",
            "cwd": dir.to_string_lossy(), "orchestrates":"missing"
        }),
    );
    assert!(!unknown.ok, "{unknown:?}");
    assert_eq!(worker.state.agents().count(), agents_before);

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_master_agent_add_launches_with_its_prompt_and_leaves_the_pwd_alone() {
    let (mut worker, room, _agent, dir) = fixture();
    worker.transport = Box::new(NoTabs);
    let pwd = dir.join("repo");
    std::fs::create_dir(&pwd).unwrap();
    let added = call(
        &mut worker,
        "add-orch",
        "agent.add",
        json!({
            "room":"master","name":"orch","provider":"claude",
            "cwd": pwd.to_string_lossy(), "orchestrates":"test"
        }),
    );
    // The agent exists and its launch is prepared; only its terminal was refused.
    assert!(
        error_message(&added).contains("no tabs in tests"),
        "{added:?}"
    );
    let orch = worker.state.agents().find(|a| a.name == "orch").unwrap().id;
    assert_eq!(worker.state.agent(orch).unwrap().orchestrates, Some(room));
    assert_eq!(std::fs::read_dir(&pwd).unwrap().count(), 0, "PWD untouched");
    let prompt =
        std::fs::read_to_string(launch_spool(&worker, orch).join("system-prompt.md")).unwrap();
    assert!(
        prompt.contains(&format!(
            "Your room: test (id {}). Your agent name: orch.",
            room.0
        )),
        "{prompt}"
    );
    assert!(prompt.contains(&format!(
        "{}/workflow-create.md",
        dir.join("docs").display()
    )));
    assert!(dir.join("docs/workflow-create.md").is_file());
    assert!(dir.join("docs/orchestrator-guide.md").is_file());
    assert!(
        messages_to(&worker, orch).is_empty(),
        "Claude takes a launch option"
    );

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_work_agent_add_rejects_an_orchestrator_prompt() {
    let (mut worker, _room, _agent, dir) = fixture();
    let rejected = call(
        &mut worker,
        "add-work-prompt",
        "agent.add",
        json!({"room":"test","name":"w","provider":"codex",
            "cwd": dir.to_string_lossy(), "system_prompt":"x"}),
    );
    assert!(
        error_message(&rejected).contains("only to MASTER"),
        "{rejected:?}"
    );

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_agent_add_adopts_an_existing_session_once_and_prompts_it_the_reviewed_way() {
    let (mut worker, room, agent, dir) = fixture();
    worker.transport = Box::new(NoTabs);
    let session = "160d1f8b-9023-44b8-9bc7-24333effb185";
    let added = call(
        &mut worker,
        "adopt-claude",
        "agent.add",
        json!({
            "room":"master","name":"orch","provider":"claude","orchestrates":"test",
            "cwd": dir.to_string_lossy(), "extra_args": format!("--model sonnet --resume {session}")
        }),
    );
    assert!(
        error_message(&added).contains("no tabs in tests"),
        "{added:?}"
    );
    let orch = worker.state.agents().find(|a| a.name == "orch").unwrap().id;
    let spool = launch_spool(&worker, orch);
    assert!(spool.join("adopted-session").is_file());
    assert_eq!(
        crate::messaging::orchestration::resume_prompt_args(Provider::ClaudeCode, &spool).unwrap()
            [..2],
        ["--system-prompt-snapshot".to_owned(), "off".into()]
    );
    assert!(messages_to(&worker, orch).is_empty());

    // The provider hook binds the session; nobody else may adopt it then.
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = Some(session.into());
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    let taken = call(
        &mut worker,
        "adopt-again",
        "agent.add",
        json!({
            "room":"test","name":"twin","provider":"claude",
            "cwd": dir.to_string_lossy(), "extra_args": format!("--resume {session}")
        }),
    );
    assert!(
        error_message(&taken).contains("already belongs to Bus agent codex1"),
        "{taken:?}"
    );
    assert!(!worker.state.agents().any(|a| a.name == "twin"));
    let _ = room;

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_adopted_cursor_session_is_bound_at_launch() {
    // cursor-agent --resume reports no sessionStart: its first hook is the
    // first prompt's, which Bus would never type without a bound session.
    struct Launches {
        tabs: usize,
        reported: std::sync::Arc<std::sync::Mutex<Vec<schema::PaneReportAgentSessionParams>>>,
    }
    impl Transport for Launches {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::TabCreate(_) => {
                    self.tabs += 1;
                    let mut pane = owned_pane_info();
                    pane.pane_id = format!("w1:p{}", self.tabs);
                    pane.terminal_id = format!("terminal-{}", self.tabs);
                    Ok(ResponseResult::TabCreated {
                        tab: serde_json::from_value(json!({
                            "tab_id": format!("t{}", self.tabs), "workspace_id": "w1",
                            "number": self.tabs, "label": "adopted", "focused": false,
                            "pane_count": 1, "agent_status": "idle"
                        }))
                        .unwrap(),
                        root_pane: pane,
                    })
                }
                Method::AgentStart(params) => Ok(ResponseResult::AgentStarted {
                    agent: owned_agent_info(&params.pane_id, &params.name, None),
                    argv: params.args,
                }),
                Method::PaneReportAgentSession(params) => {
                    self.reported.lock().unwrap().push(params);
                    Ok(ResponseResult::Ok {})
                }
                other => panic!("unexpected native method: {other:?}"),
            }
        }
    }
    // Outside any git checkout: Cursor hooks go to the project root, and a test
    // binary written into this repository's hooks would capture live agents.
    let dir = std::env::temp_dir().join(format!(
        "bus-cursor-adoption-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns()
    ));
    crate::messaging::storage::io::private_dir(&dir).unwrap();
    let reported = std::sync::Arc::default();
    let transport = Launches {
        tabs: 0,
        reported: std::sync::Arc::clone(&reported),
    };
    let mut worker = Worker::open(dir.clone(), Box::new(transport)).unwrap();
    worker.dev_enabled = true;
    let room = worker.state.create_room("adoption").unwrap();
    let session = "4915eda8-714c-4563-8041-632aea0e3325";
    for (name, provider, args) in [
        ("adopted", "cursor", format!("--resume {session}")),
        ("fresh", "cursor", String::new()),
        (
            "claude",
            "claude",
            "--resume 160d1f8b-9023-44b8-9bc7-24333effb185".into(),
        ),
    ] {
        let added = call(
            &mut worker,
            name,
            "agent.add",
            json!({"room":room.0.to_string(),"name":name,"provider":provider,
                "consent_project_hooks":true,"cwd": dir.to_string_lossy(),"extra_args": args}),
        );
        assert!(added.ok, "{added:?}");
    }
    let agent = |name: &str| worker.state.agents().find(|a| a.name == name).unwrap();
    assert_eq!(
        agent("adopted").runtime_identity.session_id.as_deref(),
        Some(session)
    );
    // Providers that report their session at launch still attest it themselves.
    assert_eq!(agent("fresh").runtime_identity.session_id, None);
    assert_eq!(agent("claude").runtime_identity.session_id, None);
    // The native layer learns it too, as from a SessionStart hook, so dialog
    // observation and delivery accept the pane.
    let reported = reported.lock().unwrap().clone();
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert_eq!(
        Some(reported[0].pane_id.as_str()),
        agent("adopted").runtime_identity.pane_id.as_deref()
    );
    assert_eq!(reported[0].source, "herdr:cursor");
    assert_eq!(reported[0].agent_session_id.as_deref(), Some(session));

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn adopting_a_session_reserves_its_owner_before_the_first_session_callback() {
    struct SuccessfulLaunches {
        tabs: usize,
    }
    impl Transport for SuccessfulLaunches {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::TabCreate(_) => {
                    self.tabs += 1;
                    let mut pane = owned_pane_info();
                    pane.pane_id = format!("w1:p{}", self.tabs);
                    pane.terminal_id = format!("terminal-{}", self.tabs);
                    Ok(ResponseResult::TabCreated {
                        tab: serde_json::from_value(json!({
                            "tab_id": format!("t{}", self.tabs), "workspace_id": "w1",
                            "number": self.tabs, "label": "adopted", "focused": false,
                            "pane_count": 1, "agent_status": "idle"
                        }))
                        .unwrap(),
                        root_pane: pane,
                    })
                }
                Method::AgentStart(params) => Ok(ResponseResult::AgentStarted {
                    agent: owned_agent_info(&params.pane_id, &params.name, None),
                    argv: params.args,
                }),
                other => panic!("unexpected native method: {other:?}"),
            }
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "bus-adoption-reservation-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns()
    ));
    crate::messaging::storage::io::private_dir(&dir).unwrap();
    let mut worker = Worker::open(dir.clone(), Box::new(SuccessfulLaunches { tabs: 0 })).unwrap();
    worker.dev_enabled = true;
    let room = worker.state.create_room("adoption").unwrap();
    let session = "160d1f8b-9023-44b8-9bc7-24333effb185";
    let params = |name| {
        json!({
            "room":room.0.to_string(), "name":name, "provider":"claude",
            "cwd":dir.to_string_lossy(), "extra_args":format!("--resume {session}")
        })
    };
    let first = call(&mut worker, "adopt-first", "agent.add", params("first"));
    assert!(first.ok, "{first:?}");
    assert_eq!(first.result["stage"], "launching");

    // The first launch has succeeded, but its SessionStart hook has not arrived.
    // A second distinct command must not start another writer to that transcript.
    let second = call(&mut worker, "adopt-second", "agent.add", params("second"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        !second.ok && error_message(&second).contains("already belongs"),
        "a successful pending adoption must reserve its provider session: {second:?}"
    );
}

#[test]
fn agent_clear_types_each_providers_reset_command_outside_bus_messages() {
    use std::sync::{Arc, Mutex};
    struct Prompts(Arc<Mutex<Vec<String>>>);
    impl Transport for Prompts {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let Method::AgentPromptIfIdle(params) = method else {
                panic!("agent clear must only type into the idle agent");
            };
            assert_eq!(params.expected_session_id, "session");
            self.0.lock().unwrap().push(params.text);
            let agent = serde_json::from_value(json!({
                "terminal_id":"terminal", "agent":"claude", "agent_status":"idle",
                "workspace_id":"workspace", "tab_id":"tab", "pane_id":"pane",
                "focused":false, "interactive_ready":true, "revision":1
            }))
            .unwrap();
            Ok(ResponseResult::AgentPrompted { agent })
        }
    }
    for (provider, command) in [
        (Provider::ClaudeCode, "/clear"),
        (Provider::Codex, "/clear"),
        (Provider::Cursor, "/new-chat"),
    ] {
        let dir = std::env::temp_dir().join(format!(
            "bus-control-clear-{}-{}-{}",
            std::process::id(),
            crate::messaging::storage::io::now_ns(),
            NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let typed = Arc::new(Mutex::new(Vec::new()));
        let mut worker = Worker::open(dir.clone(), Box::new(Prompts(Arc::clone(&typed)))).unwrap();
        worker.dev_enabled = true;
        let room = worker.state.create_room("test").unwrap();
        let agent = worker
            .state
            .create_agent(room, "worker", provider, dir.clone(), None)
            .unwrap();
        worker
            .state
            .set_agent_runtime_identity(
                agent,
                AgentRuntimeIdentity {
                    launch_id: Some("launch".into()),
                    terminal_id: Some("terminal".into()),
                    pane_id: Some("pane".into()),
                    session_id: Some("session".into()),
                },
            )
            .unwrap();
        worker.state.confirm_hook_setup(agent).unwrap();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, 1)
            .unwrap();
        let response = call(
            &mut worker,
            "clear",
            "agent.clear",
            json!({"agent":"worker"}),
        );
        assert!(response.ok, "{response:?}");
        assert_eq!(response.result["sent"], command);
        assert_eq!(*typed.lock().unwrap(), [command]);
        assert!(worker.state.agent(agent).unwrap().session_reset_pending);
        assert_eq!(worker.state.requests().count(), 0);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn agent_clear_refuses_an_agent_that_is_not_idle_or_still_has_messages() {
    let (mut worker, room, agent, dir) = fixture();
    let launching = call(
        &mut worker,
        "launching",
        "agent.clear",
        json!({"agent":"codex1"}),
    );
    assert!(
        error_message(&launching).contains("idle, ready"),
        "{launching:?}"
    );
    worker.state.confirm_hook_setup(agent).unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 1)
        .unwrap();
    worker.state.set_draft_text(room, "pending").unwrap();
    worker.state.set_draft_recipients(room, [agent]).unwrap();
    worker.state.submit_draft(room, 2).unwrap();
    let queued = call(
        &mut worker,
        "queued",
        "agent.clear",
        json!({"agent":"codex1"}),
    );
    assert!(
        error_message(&queued).contains("messages to answer"),
        "{queued:?}"
    );
    assert!(!worker.state.agent(agent).unwrap().session_reset_pending);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agents_cannot_take_the_reserved_human_name() {
    let (mut worker, room, codex, dir) = fixture();
    assert_eq!(
        worker
            .state
            .create_agent(room, "Human", Provider::Codex, "/repo".into(), None),
        Err(ModelError::ReservedAgentName)
    );
    assert_eq!(
        worker.state.rename_agent(codex, " human "),
        Err(ModelError::ReservedAgentName)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
