use super::*;
use crate::protocol::api::schema::{Method, ResponseResult};
use crate::{
    agents::providers::spool as callbacks,
    messaging::{
        native::{Transport, TransportError},
        storage::{io, state_store::JsonStore},
    },
};
use serde_json::json;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

#[path = "support_test.rs"]
mod support;
use support::*;
#[path = "delivery_test.rs"]
mod delivery_tests;
#[path = "queue_test.rs"]
mod queue_tests;

#[path = "resume_test.rs"]
mod resume_tests;

#[path = "binding_test.rs"]
mod binding_tests;
#[path = "persistence_test.rs"]
mod persistence_tests;
#[path = "recovery_test.rs"]
mod recovery_tests;

#[path = "compaction_test.rs"]
mod compaction_tests;

#[path = "scheduling_test.rs"]
mod scheduling_tests;

#[test]
fn claude_statusline_refresh_is_identity_bound_and_does_not_change_delivery_state() {
    for mismatch in [
        "none", "agent", "launch", "session", "provider", "missing", "corrupt",
    ] {
        let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
        let spool = dir.join("callbacks/launch");
        let before = std::fs::read(dir.join("state.json")).unwrap();
        let mut observation = json!({
            "manifest":{"agent_id":agent,"provider":"claude_code","launch_id":"launch"},
            "session_id":"session", "read_at_ms":io::now_ms(),
            "windows":{"five_hour":{"used_percent":12.5,"resets_at":io::now_ms()/1000+300,"window_minutes":300},
                       "weekly":{"used_percent":31,"resets_at":io::now_ms()/1000+10000,"window_minutes":10080}}
        });
        match mismatch {
            "agent" => observation["manifest"]["agent_id"] = json!(999),
            "launch" => observation["manifest"]["launch_id"] = json!("other"),
            "session" => observation["session_id"] = json!("other"),
            "provider" => observation["manifest"]["provider"] = json!("codex"),
            _ => {}
        }
        if mismatch == "corrupt" {
            std::fs::write(spool.join("usage.json"), b"corrupt").unwrap();
        } else if mismatch != "missing" {
            std::fs::write(
                spool.join("usage.json"),
                serde_json::to_vec(&observation).unwrap(),
            )
            .unwrap();
        }
        // Status lines redraw independently of Stop/SessionStart hooks: there
        // are deliberately no event files in this callback poll.
        worker.consume_callbacks(agent, &spool).unwrap();
        worker.dev_enabled = true;
        let response = worker.control_response_with_events(
            &crate::messaging::control::Request {
                id: "usage-state".into(),
                method: "state".into(),
                params: json!({}),
            },
            None,
        );
        assert!(response.ok, "{response:?}");
        let usage = &response.result["usage"]["claude"];
        if mismatch == "none" {
            assert_eq!(usage["status"], "observed");
            assert_eq!(usage["five_hour"]["used_percent"], 12.5);
            assert_eq!(usage["weekly"]["used_percent"], 31.0);
            assert_eq!(usage["observed_by_agent"], json!(agent));
        } else {
            assert_eq!(usage["status"], "unknown", "{mismatch}");
        }
        assert_eq!(std::fs::read(dir.join("state.json")).unwrap(), before);
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn compact_session_start_counts_one_compaction_per_hook() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    // Each batch is consumed before the next, as the worker does between
    // compactions; identical payloads still spooled together collapse to one.
    for batch in [
        &["startup", "compact", "compact"][..],
        &["resume"],
        &["compact"],
    ] {
        for source in batch {
            record(
                &dir,
                Provider::ClaudeCode,
                json!({"hook_event_name":"SessionStart","session_id":"session","source":source}),
            );
        }
        // A missing source (older providers) is not a compaction.
        record(
            &dir,
            Provider::ClaudeCode,
            json!({"hook_event_name":"SessionStart","session_id":"session"}),
        );
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    let compactions = worker.state.agent(agent).unwrap().compactions;
    assert_eq!(compactions.count, 2);
    assert!(compactions.last_at_ms.is_some());
    drop(worker);
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved.agent(agent).unwrap().compactions, compactions);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_final_refreshes_usage_from_its_rollout_and_state_reports_it() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker.dev_enabled = true;
    let state = |worker: &mut Worker| {
        worker
            .control_response_with_events(
                &crate::messaging::control::Request {
                    id: format!("state-{}", io::now_ns()),
                    method: "state".into(),
                    params: json!({}),
                },
                None,
            )
            .result
    };
    assert_eq!(state(&mut worker)["usage"]["codex"]["status"], "unknown");
    assert_eq!(state(&mut worker)["usage"]["claude"]["status"], "unknown");

    let rollout = dir.join("rollout-2026-10-05T00-00-00-x.jsonl");
    std::fs::write(
        &rollout,
        json!({"type":"event_msg","payload":{"type":"token_count","rate_limits":{
            "primary":{"used_percent":12.0,"window_minutes":10080,"resets_at":1791788174},
            "secondary":{"used_percent":55.5,"window_minutes":300,"resets_at":1791700000}}}})
        .to_string(),
    )
    .unwrap();
    queue(&mut worker, room, agent, "work");
    worker.submit_ready().unwrap();
    let transcript = rollout.display().to_string();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"work","transcript_path":transcript}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"done","transcript_path":transcript}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();

    let usage = &state(&mut worker)["usage"]["codex"];
    assert_eq!(usage["status"], "observed");
    assert_eq!(usage["five_hour"]["used_percent"], 55.5);
    assert_eq!(usage["weekly"]["used_percent"], 12.0);
    assert_eq!(usage["weekly"]["resets_at"], 1791788174);
    assert_eq!(usage["observed_by_agent"], json!(agent));
    assert!(usage["read_at_ms"].as_u64().is_some());

    // A later unreadable rollout keeps the last snapshot instead of erroring.
    std::fs::remove_file(&rollout).unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn2","last_assistant_message":"again","transcript_path":transcript}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        state(&mut worker)["usage"]["codex"]["weekly"]["used_percent"],
        12.0
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

mod status_tests {
    //! Agent availability as the pane listing reports it.

    use super::*;

    /// One `agent.list` answer for the fixture's pane: the provider identified
    /// as `agent` (None once the pane runs something else) with its screen
    /// status, or no entry at all once the pane is no longer an agent.
    fn pane_listing(agent: Option<&str>, status: &str) -> Result<ResponseResult, TransportError> {
        let info: crate::protocol::api::schema::AgentInfo = serde_json::from_value(json!({
            "terminal_id": "terminal", "pane_id": "pane", "agent": agent,
            "agent_status": status, "agent_session": agent.map(|agent| json!({
                "source": format!("herdr:{agent}"), "agent": agent, "kind": "id", "value": "session"
            })),
            "workspace_id": "w1", "tab_id": "t1", "focused": false,
            "interactive_ready": true, "revision": 1
        }))
        .unwrap();
        Ok(ResponseResult::AgentList { agents: vec![info] })
    }

    #[test]
    fn a_failed_self_update_shows_its_error_until_the_chooser_is_answered() {
        let error = "Bus could not install the codex update (npm failed). Answer the update prompt in bus-r1-a2's terminal; queued messages wait until then.";
        let mut failed = pane_listing(Some("codex"), "blocked");
        if let Ok(ResponseResult::AgentList { agents }) = &mut failed {
            agents[0].dialog_id = Some("chooser".into());
            agents[0].update_error = Some(error.into());
        }
        let failed_again = failed.clone();
        let (mut worker, agent, room, dir, _) = fixture(
            Provider::Codex,
            vec![failed, failed_again, pane_listing(Some("codex"), "idle")],
        );
        let shown = |worker: &Worker| worker.state.agent(agent).unwrap().actionable_error.clone();
        worker.poll().unwrap();
        assert_eq!(shown(&worker).as_deref(), Some(error));
        let request = queue(&mut worker, room, agent, "waits for the chooser");
        worker.poll().unwrap();
        assert_eq!(shown(&worker).as_deref(), Some(error));
        assert_eq!(
            worker
                .state
                .stall_reason(worker.state.request(request).unwrap(), u64::MAX),
            None
        );
        worker.poll().unwrap();
        assert_eq!(shown(&worker), None);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_agent_whose_provider_is_not_running_is_unavailable_until_it_returns() {
        let no_agents = Ok(ResponseResult::AgentList { agents: Vec::new() });
        let (mut worker, agent, room, dir, calls) = fixture(
            Provider::Codex,
            vec![
                pane_listing(Some("codex"), "idle"),
                // Codex's own updater replaced its UI: the screen is not Codex.
                pane_listing(Some("codex"), "unknown"),
                // The provider exited and the shell is back.
                no_agents,
                // Another program holds the pane.
                pane_listing(None, "unknown"),
                // Relaunched.
                pane_listing(Some("codex"), "idle"),
            ],
        );
        let status = |worker: &Worker| worker.state.agent(agent).unwrap().status;
        worker.poll().unwrap();
        assert_eq!(status(&worker), RuntimeStatus::Idle);
        let request = queue(&mut worker, room, agent, "held while unavailable");
        for case in ["updater", "exited", "foreign program"] {
            worker.poll().unwrap();
            assert_eq!(status(&worker), RuntimeStatus::Unavailable, "{case}");
            assert_eq!(
                worker.state.room_status(room),
                RuntimeStatus::Blocked,
                "{case}"
            );
            assert_eq!(
                crate::messaging::diagnostics::wait_reason(worker.state.agent(agent).unwrap()),
                Some("agent_unavailable"),
                "{case}"
            );
            worker.submit_ready().unwrap();
            assert_eq!(
                worker.state.request(request).unwrap().phase,
                RequestPhase::Queued,
                "{case}: the delivery stays queued"
            );
        }
        assert!(!calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("agent.prompt")));
        worker.poll().unwrap();
        assert_eq!(status(&worker), RuntimeStatus::Idle);
        assert_eq!(worker.state.room_status(room), RuntimeStatus::Idle);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

mod update_lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn slow_startup_update_preserves_queued_messages_until_it_finishes() {
        use crate::agents::{AgentKind, AgentState};
        use crate::server::app::{App, AppPolicy};
        use crate::server::workspaces::Workspace;
        use crate::terminal::events::TerminalEvent;
        let (_, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::utils::config::Config::default(),
            AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("update")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(120, 40, 16);
        app.terminal_runtimes.insert(terminal_id.clone(), runtime);
        let public_pane = app.public_pane_id(0, pane).unwrap();
        let start = app.handle_api_request(crate::protocol::api::schema::Request {
            id: "start".into(),
            method: Method::AgentStart(crate::protocol::api::schema::AgentStartParams {
                name: "bus-r1-a2".into(),
                kind: "codex".into(),
                pane_id: public_pane,
                args: vec![],
                timeout_ms: Some(4000),
            }),
        });
        assert!(start.contains("agent_started"), "{start}");
        writes.try_recv().unwrap();
        app.terminal_runtimes
            .get(&terminal_id)
            .unwrap()
            .test_process_pty_bytes(include_bytes!(
                "../../../../tests/fixtures/codex-update/chooser-0.162.ansi"
            ));
        let now = std::time::Instant::now();
        let event = |state, blocker| TerminalEvent::StateChanged {
            pane_id: pane,
            agent: Some(AgentKind::Codex),
            state,
            visible_blocker: blocker,
            process_exited: false,
            exited_process: None,
            observed_at: now,
        };
        app.state.handle_app_event(event(AgentState::Blocked, true));
        assert!(app.supervise_self_updates(now));
        assert_eq!(writes.try_recv().unwrap(), bytes::Bytes::from_static(b"\r"));
        app.terminal_runtimes
            .get(&terminal_id)
            .unwrap()
            .test_process_pty_bytes(
                b"\x1b[2J\x1b[HUpdating Codex via `npm install -g @openai/codex`...\r\n",
            );
        app.state
            .handle_app_event(event(AgentState::Working, false));
        app.supervise_self_updates(now);
        let listing = app.handle_api_request(crate::protocol::api::schema::Request {
            id: "list".into(),
            method: Method::AgentList(crate::protocol::api::schema::EmptyParams {}),
        });
        let response: crate::protocol::api::schema::SuccessResponse =
            serde_json::from_str(&listing).unwrap();
        let ResponseResult::AgentList { agents } = &response.result else {
            panic!("{listing}")
        };
        let info = &agents[0];
        assert_eq!(
            info.agent_status,
            crate::protocol::api::schema::AgentStatus::Working
        );
        assert!(info.dialog_id.is_none());
        let identity = AgentRuntimeIdentity {
            launch_id: Some("launch".into()),
            terminal_id: Some(info.terminal_id.clone()),
            pane_id: Some(info.pane_id.clone()),
            session_id: None,
        };
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![Ok(response.result)]);
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        worker.save(worker.state.clone()).unwrap();
        worker.poll().unwrap();
        let request = queue(&mut worker, room, agent, "brief waits for the update");
        let expired = worker
            .state
            .expire_stalled_queued_requests(io::now_ms() + 180_000)
            .unwrap();
        let phase = worker.state.request(request).unwrap().phase;
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            expired.is_empty(),
            "slow install expired queued requests: {expired:?}; phase={phase:?}"
        );
        assert_eq!(phase, RequestPhase::Queued);
    }

    #[test]
    fn recovered_update_error_clears_after_coordinator_restart() {
        let error = "Bus could not install the codex update (npm failed). Answer the update prompt in bus-r1-a2's terminal; queued messages wait until then.";
        let listing = |failed: bool| {
            let info: crate::protocol::api::schema::AgentInfo = serde_json::from_value(json!({
                "terminal_id": "terminal", "pane_id": "pane", "agent": "codex",
                "agent_status": if failed { "blocked" } else { "idle" },
                "agent_session": {"source":"herdr:codex","agent":"codex","kind":"id","value":"session"},
                "workspace_id":"w1","tab_id":"t1","focused":false,"interactive_ready":true,"revision":1,
                "dialog_id": if failed {Some("chooser")} else {None},
                "update_error": if failed {Some(error)} else {None},
            })).unwrap();
            Ok(ResponseResult::AgentList { agents: vec![info] })
        };
        let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![listing(true)]);
        worker.poll().unwrap();
        assert_eq!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .actionable_error
                .as_deref(),
            Some(error)
        );
        worker.save(worker.state.clone()).unwrap();
        drop(worker);
        let fake = FakeTransport {
            replies: vec![listing(false)].into(),
            calls,
            state_path: dir.join("state.json"),
        };
        let mut reopened = Worker::open(dir.clone(), Box::new(fake)).unwrap();
        reopened.poll().unwrap();
        assert_eq!(
            reopened.state.agent(agent).unwrap().status,
            RuntimeStatus::Idle
        );
        let actual = reopened
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .clone();
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(
            actual, None,
            "recovered agent must not retain a stale update failure"
        );
    }
}
