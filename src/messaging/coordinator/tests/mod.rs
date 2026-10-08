use super::*;
use crate::api::schema::{Method, ResponseResult};
use crate::bus::{
    callbacks, io,
    store::JsonStore,
    transport::{Transport, TransportError},
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

#[path = "status.rs"]
mod status_tests;

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
        let response = worker.dev_response_with_events(
            &crate::bus::control::Request {
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
fn delete_agent_stops_terminal_before_removing_persisted_work() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "discard me");
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert!(worker.state.request(request).is_none());
    assert!(worker
        .state
        .room(room)
        .unwrap()
        .draft
        .recipient_ids
        .is_empty());
    assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
    worker.submit_ready().unwrap();
    drop(worker);
    let recovered = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert!(recovered.agent(agent).is_none());
    assert!(recovered.request(request).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_failure_suspends_delivery_across_restart_and_allows_retry() {
    for ownership_lookup in [
        native_agents(Some("bus-r1-a2".into())),
        Err(TransportError {
            message: "server busy".into(),
            code: None,
            definitely_rejected: false,
        }),
    ] {
        delete_failure_suspends_while_native_ownership_is_unproven(ownership_lookup);
    }
}

/// Regression: a Codex agent whose pane respawned after a Bus restart lost its
/// native managed name, so every guarded close answered
/// `terminal_identity_changed` and deletion stayed suspended forever.
#[test]
fn delete_finishes_and_reports_terminal_left_open_when_native_ownership_was_released() {
    for delete_room in [false, true] {
        let (mut worker, agent, room, dir, calls) = fixture(
            Provider::Codex,
            vec![identity_changed(), native_agents(None)],
        );
        let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        worker.save(worker.state.clone()).unwrap();
        let request = queue(&mut worker, room, agent, "stranded at awaiting_start");
        let (events, received) = mpsc::channel();
        let command = if delete_room {
            BusCommand::DeleteRoom(room)
        } else {
            BusCommand::DeleteAgent(agent)
        };
        worker.command(command, &events).unwrap();
        assert!(worker.state.agent(agent).is_none());
        assert!(worker.state.request(request).is_none());
        assert_eq!(worker.state.room(room).is_none(), delete_room);
        // Never a second, unguarded close: the pane now holds foreign work.
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["pane.close_if_identity", "agent.list"]
        );
        let left_open = vec![LeftOpenTerminal {
            agent_id: agent,
            agent_name: "author".into(),
            pane_id: "pane".into(),
            terminal_id: "terminal".into(),
        }];
        assert!(received
            .try_iter()
            .any(|event| matches!(&event, BusEvent::TerminalsLeftOpen(t) if *t == left_open)));
        let report = worker.snapshot().error.unwrap();
        assert!(
            report.contains("\"author\"") && report.contains("pane pane, terminal"),
            "{report}"
        );
        drop(worker);
        let recovered = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert!(recovered.agent(agent).is_none());
        assert!(recovered.request(request).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn delete_room_partial_close_preserves_retryable_state_and_other_rooms() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![
            Ok(ResponseResult::Ok {}),
            Err(TransportError {
                message: "stop failed".into(),
                code: Some("terminal_stop_failed".into()),
                definitely_rejected: false,
            }),
        ],
    );
    let project = dir.join("project-file.txt");
    std::fs::write(&project, "keep project data").unwrap();
    let other_room = worker.state.create_room("other").unwrap();
    let other_agent = worker
        .state
        .create_agent(other_room, "unrelated", Provider::Codex, dir.clone(), None)
        .unwrap();
    let second = worker
        .state
        .create_agent(room, "second", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker
        .state
        .set_agent_runtime_identity(
            second,
            AgentRuntimeIdentity {
                launch_id: Some("second-launch".into()),
                terminal_id: Some("second-terminal".into()),
                pane_id: Some("second-pane".into()),
                session_id: Some("second-session".into()),
            },
        )
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "remove");
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteRoom(room), &events)
        .is_err());
    assert!(worker.state.room(room).unwrap().deletion_pending);
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(worker.state.agent(second).unwrap().deletion_pending);
    assert!(!worker.state.agent(other_agent).unwrap().deletion_pending);
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 2);
    drop(worker);
    let fake = FakeTransport {
        replies: VecDeque::new(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .command(BusCommand::DeleteRoom(room), &events)
        .unwrap();
    assert!(recovered.state.room(room).is_none());
    assert!(recovered.state.agent(agent).is_none());
    assert!(recovered.state.agent(second).is_none());
    assert!(recovered.state.request(request).is_none());
    assert!(recovered.state.agent(other_agent).is_some());
    assert!(recovered.state.room(other_room).is_some());
    assert_eq!(
        std::fs::read_to_string(project).unwrap(),
        "keep project data"
    );
    assert_eq!(calls.lock().unwrap().len(), 4);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_unknown_launch_identity_fails_closed_without_transport() {
    let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![]);
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("uncertain-launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(calls.lock().unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deletion_retry_reconciles_launch_attested_initial_session_without_callbacks_or_delivery() {
    for provider in [Provider::ClaudeCode, Provider::Cursor, Provider::Codex] {
        let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
        let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        queue(&mut worker, room, agent, "never deliver");
        let calls = Arc::new(Mutex::new(Vec::new()));
        worker.transport = Box::new(NativeBoundClose {
            session: "native-session".into(),
            calls: Arc::clone(&calls),
            state_path: dir.join("state.json"),
        });
        let (events, _) = mpsc::channel();
        assert!(worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .is_err());
        assert!(worker.state.agent(agent).unwrap().deletion_pending);
        record(
            &dir,
            provider,
            json!({"hook_event_name":if provider == Provider::Cursor {"sessionStart"} else {"SessionStart"}, "session_id":"native-session", "conversation_id":"native-session"}),
        );
        record(
            &dir,
            provider,
            json!({"hook_event_name":if provider == Provider::Cursor {"beforeSubmitPrompt"} else {"UserPromptSubmit"}, "session_id":"native-session", "conversation_id":"native-session", "prompt_id":"p", "turn_id":"p", "generation_id":"p", "prompt":"never deliver"}),
        );
        record(
            &dir,
            provider,
            json!({"hook_event_name":if provider == Provider::Cursor {"afterAgentResponse"} else {"Stop"}, "session_id":"native-session", "conversation_id":"native-session", "prompt_id":"p", "turn_id":"p", "generation_id":"p", "last_assistant_message":"never publish", "text":"never publish"}),
        );
        worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .unwrap();
        assert!(worker.state.agent(agent).is_none());
        assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
        assert_eq!(worker.state.requests().count(), 0);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![None, None, Some("native-session".into())]
        );
        assert_eq!(
            callbacks::records(&dir.join("callbacks/launch"))
                .unwrap()
                .len(),
            3
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn deletion_initial_session_reconciliation_rejects_conflicting_launch_sessions() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.state.prepare_delete_agent(agent).unwrap();
    worker.save(worker.state.clone()).unwrap();
    for session in ["first", "second"] {
        record(
            &dir,
            Provider::ClaudeCode,
            json!({"hook_event_name":"SessionStart", "session_id":session}),
        );
    }
    assert!(worker
        .reconcile_deleting_initial_session(agent)
        .unwrap_err()
        .contains("Conflicting"));
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .session_id
        .is_none());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deletion_never_rebinds_a_known_session_from_a_later_session_start() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(NativeBoundClose {
        session: "rebound".into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    });
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart", "session_id":"rebound"}),
    );
    let (events, received) = mpsc::channel();
    // The agent is deleted, but its terminal is never closed under the
    // rebound session: that session may not be Bus's to end.
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert_eq!(*calls.lock().unwrap(), vec![Some("session".into())]);
    assert!(received
        .try_iter()
        .any(|event| matches!(event, BusEvent::TerminalsLeftOpen(_))));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unsupported_native_close_requires_server_restart_and_never_falls_back() {
    for code in ["unknown_method", "invalid_request"] {
        let (mut worker, agent, _, dir, calls) = fixture(
            Provider::Codex,
            vec![Err(TransportError {
                message: "RAW_API_DETAIL unknown_method: pane.close_if_identity; expected one of many API variants".repeat(20),
                code: Some(code.into()),
                definitely_rejected: true,
            })],
        );
        let (events, _) = mpsc::channel();
        let error = worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .unwrap_err();
        assert!(
            error.contains("Update and restart the Bus server"),
            "{error}"
        );
        assert!(error.len() < 220, "user-facing error must remain compact");
        assert!(!error.contains("RAW_API_DETAIL"));
        assert!(worker.state.agent(agent).unwrap().deletion_pending);
        assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn deletion_transport_details_stay_out_of_user_errors() {
    for result in [
        Err(TransportError {
            message: "RAW_API_DETAIL terminal driver details".repeat(100),
            code: Some("terminal_stop_failed".into()),
            definitely_rejected: false,
        }),
        Ok(ResponseResult::AgentList { agents: vec![] }),
    ] {
        let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![result]);
        let (events, _) = mpsc::channel();
        let error = worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .unwrap_err();
        assert!(error.len() < 220, "{error}");
        assert!(
            !error.contains("RAW_API_DETAIL") && !error.contains("AgentList"),
            "{error}"
        );
        assert!(worker.state.agent(agent).unwrap().deletion_pending);
        assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn successful_deletion_retry_clears_previous_worker_snapshot_error() {
    for delete_room in [false, true] {
        let (worker, agent, room, dir, _) = fixture(
            Provider::Codex,
            vec![
                Err(TransportError {
                    message: "temporary stop failure".into(),
                    code: Some("terminal_stop_failed".into()),
                    definitely_rejected: true,
                }),
                Ok(ResponseResult::Ok {}),
            ],
        );
        let deletion = if delete_room {
            BusCommand::DeleteRoom(room)
        } else {
            BusCommand::DeleteAgent(agent)
        };
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (commands, receiver) = mpsc::sync_channel(256);
        let (events, received) = mpsc::channel();
        commands.send((1, deletion.clone())).unwrap();
        commands.send((2, deletion)).unwrap();
        commands.send((3, BusCommand::Shutdown)).unwrap();
        worker.run(receiver, events, Arc::clone(&snapshots));
        let outcomes: Vec<_> = received
            .try_iter()
            .filter_map(|event| match event {
                BusEvent::CommandFinished { result, .. } => Some(result),
                _ => None,
            })
            .collect();
        assert!(outcomes[0].is_err());
        assert!(outcomes[1].is_ok());
        assert!(snapshots.lock().unwrap().error.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn delete_last_room_stays_empty_after_restart_and_ignores_late_callbacks() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "deleted request");
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteRoom(room), &events)
        .unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"SessionStart", "session_id":"late-session"}),
    );
    drop(worker);
    let fake = FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![] })].into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered.tick().unwrap();
    // Only the undeletable MASTER room remains.
    assert_eq!(
        recovered
            .state
            .rooms()
            .map(|room| room.kind)
            .collect::<Vec<_>>(),
        [RoomKind::Master]
    );
    assert!(!recovered.state.has_work());
    assert_eq!(recovered.state.agents().count(), 0);
    assert_eq!(recovered.state.requests().count(), 0);
    assert!(!recovered.state.is_pristine());
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["pane.close_if_identity", "agent.list"]
    );
    let new_room = recovered.state.create_room("new room").unwrap();
    assert!(new_room.0 > agent.0);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn initial_recovery_tick_keeps_detached_room_reply_unread_until_selected() {
    // A saved visible room belongs to the detached client. The first coordinator
    // tick must not mark its newly spooled reply seen before any UI selection.
    let (mut worker, agent, room_b, dir, _) = fixture(Provider::Codex, vec![]);
    let room_a = worker.state.create_room("other room").unwrap();
    worker.state.select_room(room_b).unwrap();
    let request = queue(&mut worker, room_b, agent, "finish while detached");
    worker.state.begin_submission(request, "launch", 0).unwrap();
    worker
        .state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn".into()),
            },
        )
        .unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"finish while detached"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.request(request).unwrap().trusted_start_bound);
    assert_eq!(worker.state.room(room_b).unwrap().unread_count, 0);
    drop(worker);
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"completed while detached"}),
    );
    let info = serde_json::from_value(json!({
        "terminal_id": "terminal", "agent": "codex", "agent_status": "idle",
        "agent_session": {"source": "herdr:codex", "agent": "codex", "kind": "id", "value": "session"},
        "workspace_id": "workspace", "tab_id": "tab", "pane_id": "pane",
        "focused": false, "interactive_ready": true, "revision": 1
    }))
    .unwrap();
    let fake = FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
        calls: Arc::new(Mutex::new(Vec::new())),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered.tick().unwrap();
    assert_eq!(
        recovered.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        recovered.state.room(room_b).unwrap().latest_replies[&agent].text,
        "completed while detached"
    );
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 1);
    let (events, _) = mpsc::channel();
    recovered
        .command(BusCommand::SelectRoom(room_a), &events)
        .unwrap();
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 1);
    assert_eq!(
        recovered
            .store
            .load()
            .unwrap()
            .unwrap()
            .room(room_b)
            .unwrap()
            .unread_count,
        1
    );
    recovered
        .command(BusCommand::SelectRoom(room_b), &events)
        .unwrap();
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 0);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn storage_failure_prevents_send_and_startup_failure_releases_coordinator() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "prompt");
    io::atomic_write(&dir.join("state.json"), b"corrupt").unwrap();
    assert!(worker.submit_ready().is_err());
    assert!(calls.lock().unwrap().is_empty());
    assert!(worker.storage_failed);
    drop(worker);
    let fake = || {
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls: calls.clone(),
            state_path: dir.join("state.json"),
        })
    };
    assert!(Worker::open(dir.clone(), fake()).is_err());
    assert!(io::lock(&dir.join("coordinator.lock")).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn worker_open_gives_every_session_exactly_one_master_room() {
    let (worker, _agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let master = worker
        .state
        .master_room()
        .expect("new session has MASTER")
        .id;
    assert_ne!(master, room);
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved.master_room().map(|room| room.id), Some(master));
    drop(worker);

    let reopened = Worker::open(
        dir.clone(),
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls,
            state_path: dir.join("state.json"),
        }),
    )
    .unwrap();
    assert_eq!(
        reopened
            .state
            .rooms()
            .filter(|room| room.kind == RoomKind::Master)
            .map(|room| room.id)
            .collect::<Vec<_>>(),
        [master]
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
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
            .dev_response_with_events(
                &crate::bus::control::Request {
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

#[test]
fn master_session_saved_before_sound_and_compactions_keeps_its_orchestrators() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let work = state.create_room("pr-1").unwrap();
    let orchestrator = state
        .create_agent(master, "orch", Provider::ClaudeCode, "/repo".into(), None)
        .unwrap();
    state.bind_orchestrator(orchestrator, work).unwrap();
    let mut value = serde_json::to_value(&state).unwrap();
    for room in value["rooms"].as_object_mut().unwrap().values_mut() {
        room.as_object_mut().unwrap().remove("sound");
    }
    for agent in value["agents"].as_object_mut().unwrap().values_mut() {
        agent.as_object_mut().unwrap().remove("compactions");
    }
    value
        .as_object_mut()
        .unwrap()
        .remove("consumed_dialog_fingerprints");
    let (worker, dir) = open_saved_document(json!({"version": 1, "state": value}));
    assert_eq!(worker.state.master_room().map(|room| room.id), Some(master));
    assert_eq!(
        worker
            .state
            .rooms()
            .filter(|room| room.kind == RoomKind::Master)
            .count(),
        1
    );
    assert!(worker.state.room(master).unwrap().sound_enabled());
    assert!(!worker.state.room(work).unwrap().sound_enabled());
    let agent = worker.state.agent(orchestrator).unwrap();
    assert_eq!(agent.orchestrates, Some(work));
    assert_eq!(agent.compactions.count, 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleared_claude_rebinds_to_its_fresh_session_start() {
    let (mut worker, agent, _room, dir, calls) = fixture(Provider::ClaudeCode, vec![]);
    worker.state.begin_session_reset(agent).unwrap();
    worker.state.record_compaction(agent, 5).unwrap();
    worker.save(worker.state.clone()).unwrap();
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart","session_id":"fresh","source":"clear"}),
    );
    // The first pass rebinds; the retained SessionStart is consumed on the next.
    for _ in 0..2 {
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    let cleared = agent_of(&worker, agent);
    assert_eq!(
        cleared.runtime_identity.session_id.as_deref(),
        Some("fresh")
    );
    assert!(!cleared.session_binding_invalidated);
    assert!(!cleared.session_reset_pending);
    assert_eq!(cleared.compactions.count, 0);
    assert!(callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .is_empty());
    assert!(calls.lock().unwrap().contains(&"pane.report_agent_session"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleared_codex_answers_a_message_sent_into_its_fresh_session() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker.state.begin_session_reset(agent).unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "fresh task");
    worker.submit_ready().unwrap();
    // Codex starts the fresh session with the first turn after /clear.
    for value in [
        json!({"hook_event_name":"SessionStart","session_id":"fresh","source":"startup"}),
        json!({"hook_event_name":"UserPromptSubmit","session_id":"fresh","turn_id":"t","prompt":"fresh task"}),
        json!({"hook_event_name":"Stop","session_id":"fresh","turn_id":"t","last_assistant_message":"FRESH"}),
    ] {
        record(&dir, Provider::Codex, value);
    }
    for _ in 0..2 {
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "FRESH"
    );
    assert_eq!(
        agent_of(&worker, agent)
            .runtime_identity
            .session_id
            .as_deref(),
        Some("fresh")
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleared_cursor_rebinds_from_the_first_turn_of_its_new_chat() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Cursor, vec![]);
    worker.state.begin_session_reset(agent).unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "fresh task");
    worker.submit_ready().unwrap();
    // Cursor sends no sessionStart for /new-chat; the turn names the new chat.
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"fresh","generation_id":"t","prompt":"fresh task"}),
    );
    for _ in 0..2 {
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    let cleared = agent_of(&worker, agent);
    assert_eq!(
        cleared.runtime_identity.session_id.as_deref(),
        Some("fresh")
    );
    assert!(!cleared.session_binding_invalidated);
    let request = worker.state.request(request).unwrap();
    assert!(request.trusted_start_bound);
    assert_eq!(request.provider_session_id.as_deref(), Some("fresh"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleared_cursor_keeps_its_session_when_the_server_refuses_the_new_chat() {
    // A server older than the Cursor new-chat rule answers the report with ok
    // but still attributes the old session to the pane.
    let (mut worker, agent, _room, dir, _) = fixture(
        Provider::Cursor,
        vec![
            Ok(ResponseResult::Ok {}),
            Ok(ResponseResult::AgentInfo {
                agent: native_info("pane", "cursor", "session"),
            }),
        ],
    );
    worker.state.begin_session_reset(agent).unwrap();
    worker.save(worker.state.clone()).unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"fresh","generation_id":"t","prompt":"fresh task"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    let refused = agent_of(&worker, agent);
    assert_eq!(
        refused.runtime_identity.session_id.as_deref(),
        Some("session")
    );
    assert!(refused.session_binding_invalidated);
    assert_eq!(refused.status, RuntimeStatus::Unavailable);
    assert!(refused
        .actionable_error
        .as_deref()
        .is_some_and(|error| error.contains("kept the previous provider session")));
    assert!(callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Regression: a Cursor agent rebound to its new chat while the server kept the
/// old one; every guarded close answered `terminal_identity_changed` and the
/// agent could never be deleted.
#[test]
fn delete_leaves_open_an_owned_terminal_whose_provider_session_moved() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Cursor,
        vec![
            identity_changed(),
            native_agents(Some("bus-r1-a2".into())),
            Ok(ResponseResult::AgentInfo {
                agent: native_info("pane", "cursor", "moved"),
            }),
        ],
    );
    let request = queue(&mut worker, room, agent, "stuck");
    let (events, received) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert!(worker.state.request(request).is_none());
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["pane.close_if_identity", "agent.list", "agent.get"]
    );
    assert!(received.try_iter().any(|event| matches!(
        event,
        BusEvent::TerminalsLeftOpen(terminals) if terminals[0].pane_id == "pane"
    )));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_keeps_an_agent_whose_terminal_carries_another_managed_name() {
    let mut foreign = native_info("pane", "cursor", "moved");
    foreign.name = Some("bus-r1-a9".into());
    let (mut worker, agent, _room, dir, calls) = fixture(
        Provider::Cursor,
        vec![
            identity_changed(),
            native_agents(Some("bus-r1-a2".into())),
            Ok(ResponseResult::AgentInfo { agent: foreign }),
        ],
    );
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            "pane.close_if_identity",
            "agent.list",
            "agent.get",
            "agent.list"
        ]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Regression: after a restart the agent's managed name moved to a restored
/// terminal that kept another Cursor chat, Bus never rebound to it, and every
/// deletion failed with "The terminal session changed".
#[test]
fn delete_leaves_open_a_restored_terminal_whose_provider_session_moved() {
    let mut restored = native_info("w1:p7", "cursor", "moved");
    restored.terminal_id = "restored".into();
    let restored_list = || {
        Ok(ResponseResult::AgentList {
            agents: vec![restored.clone()],
        })
    };
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Cursor,
        vec![
            identity_changed(),
            restored_list(),
            Err(TransportError {
                message: "pane not found".into(),
                code: Some("pane_not_found".into()),
                definitely_rejected: true,
            }),
            restored_list(),
        ],
    );
    let request = queue(&mut worker, room, agent, "stuck");
    let (events, received) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert!(worker.state.request(request).is_none());
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            "pane.close_if_identity",
            "agent.list",
            "agent.get",
            "agent.list"
        ]
    );
    assert!(received.try_iter().any(|event| matches!(
        event,
        BusEvent::TerminalsLeftOpen(terminals)
            if terminals[0].pane_id == "w1:p7" && terminals[0].terminal_id == "restored"
    )));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_keeps_an_agent_whose_restored_terminal_runs_its_own_session() {
    let mut restored = native_info("w1:p7", "cursor", "session");
    restored.terminal_id = "restored".into();
    let restored_list = || {
        Ok(ResponseResult::AgentList {
            agents: vec![restored.clone()],
        })
    };
    let (mut worker, agent, _room, dir, _) = fixture(
        Provider::Cursor,
        vec![
            identity_changed(),
            restored_list(),
            Err(TransportError {
                message: "pane not found".into(),
                code: Some("pane_not_found".into()),
                definitely_rejected: true,
            }),
            restored_list(),
        ],
    );
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).is_some());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
