use super::*;
use crate::bus::transport::TransportError;
use serde_json::json;

struct NoTransport;
impl Transport for NoTransport {
    fn request(&mut self, _method: Method) -> Result<ResponseResult, TransportError> {
        panic!("Domain-only command unexpectedly reached native transport")
    }
}

fn fixture() -> (Worker, RoomId, AgentId, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "bus-control-domain-{}-{}",
        std::process::id(),
        super::super::super::io::now_ns()
    ));
    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    worker.dev_enabled = true;
    let room = worker.state.create_room("test").unwrap();
    let agent = worker
        .state
        .create_agent(room, "codex1", Provider::Codex, dir.clone(), None)
        .unwrap();
    (worker, room, agent, dir)
}

fn call(worker: &mut Worker, id: &str, method: &str, params: serde_json::Value) -> Response {
    worker.dev_response(&ControlRequest {
        id: id.into(),
        method: method.into(),
        params,
        capability: None,
    })
}

const CONTROL_TOKEN: &str = "Rk9vQmFyOTdaeDNRd0x1TnBFc1R2MmhKZGtDeQ";

fn call_with_capability(
    worker: &mut Worker,
    id: &str,
    method: &str,
    params: serde_json::Value,
    capability: Option<&str>,
) -> Response {
    worker.dev_response(&ControlRequest {
        id: id.into(),
        method: method.into(),
        params,
        capability: capability.map(str::to_owned),
    })
}

fn protected_fixture() -> (Worker, RoomId, AgentId, PathBuf) {
    let (mut worker, room, agent, dir) = fixture();
    worker.control_capability =
        Some(crate::bus::orchestrator_control::validate_and_hash(CONTROL_TOKEN).unwrap());
    (worker, room, agent, dir)
}

#[test]
fn room_orchestrator_control_protected_server_rejects_a_missing_capability() {
    let (mut worker, _room, _agent, dir) = protected_fixture();
    let before = worker.state.rooms().count();

    let response = call(
        &mut worker,
        "unauthorized-1",
        "room.create",
        json!({"name":"x"}),
    );

    assert!(!response.ok, "{response:?}");
    assert_eq!(response.error.unwrap().code, "capability_required");
    assert_eq!(
        worker.state.rooms().count(),
        before,
        "unauthorized request mutated state"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn room_orchestrator_control_protected_server_rejects_a_wrong_capability_without_leaking_it() {
    let (mut worker, _room, _agent, dir) = protected_fixture();
    let before = worker.state.rooms().count();

    let response = call_with_capability(
        &mut worker,
        "unauthorized-2",
        "room.create",
        json!({"name":"x"}),
        Some("Rk9vQmFyOTdaeDNRd0x1TnBFc1R2MmhKZGtDeX"),
    );

    assert!(!response.ok, "{response:?}");
    let error = response.error.unwrap();
    assert_eq!(error.code, "capability_invalid");
    assert!(!error.message.contains(CONTROL_TOKEN), "{error:?}");
    assert!(!error.message.contains("Rk9vQmFy"), "{error:?}");
    assert_eq!(
        worker.state.rooms().count(),
        before,
        "unauthorized request mutated state"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn room_orchestrator_control_rejects_an_unauthorized_request_before_method_dispatch() {
    let (mut worker, room, _agent, dir) = protected_fixture();
    let name = worker.state.room(room).unwrap().name.clone();

    // Authorization precedes every later gate: a missing --confirm would otherwise
    // answer confirmation_required, and an unknown method unknown_method.
    let deletion = call(
        &mut worker,
        "unauthorized-3",
        "room.delete",
        json!({"room": name, "confirm": false}),
    );
    let unknown = call(&mut worker, "unauthorized-4", "room.detonate", json!({}));

    assert_eq!(deletion.error.unwrap().code, "capability_required");
    assert_eq!(unknown.error.unwrap().code, "capability_required");
    assert!(worker.state.room(room).is_some(), "room was deleted");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn room_orchestrator_control_protected_server_accepts_the_launch_capability() {
    let (mut worker, _room, _agent, dir) = protected_fixture();
    let before = worker.state.rooms().count();

    let response = call_with_capability(
        &mut worker,
        "authorized-1",
        "room.create",
        json!({"name":"authorized"}),
        Some(CONTROL_TOKEN),
    );

    assert!(response.ok, "{response:?}");
    assert_eq!(worker.state.rooms().count(), before + 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn room_orchestrator_control_rejection_leaves_no_receipt_for_a_later_authorized_retry() {
    let (mut worker, _room, _agent, dir) = protected_fixture();

    let rejected = call(
        &mut worker,
        "retry-1",
        "room.create",
        json!({"name":"retry"}),
    );
    let accepted = call_with_capability(
        &mut worker,
        "retry-1",
        "room.create",
        json!({"name":"retry"}),
        Some(CONTROL_TOKEN),
    );

    assert!(!rejected.ok, "{rejected:?}");
    assert!(accepted.ok, "{accepted:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn room_orchestrator_control_only_the_first_worker_adopts_the_launch_capability() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    std::env::set_var(
        crate::bus::orchestrator_control::TOKEN_ENV_VAR,
        CONTROL_TOKEN,
    );
    crate::bus::orchestrator_control::arm_from_environment().unwrap();

    let (mut first, _first_room, _first_agent, first_dir) = fixture();
    first.adopt_launch_capability();
    let (mut second, _second_room, _second_agent, second_dir) = fixture();
    second.adopt_launch_capability();

    let protected = call(&mut first, "once-1", "room.create", json!({"name":"x"}));
    let not_inherited = call(&mut second, "once-2", "room.create", json!({"name":"y"}));

    assert_eq!(protected.error.unwrap().code, "capability_required");
    assert!(
        not_inherited.ok,
        "a second worker inherited the launch capability: {not_inherited:?}"
    );

    crate::bus::orchestrator_control::disarm_for_test();
    std::env::remove_var(crate::bus::orchestrator_control::TOKEN_ENV_VAR);
    let _ = std::fs::remove_dir_all(first_dir);
    let _ = std::fs::remove_dir_all(second_dir);
}

#[test]
fn room_orchestrator_control_receipts_never_retain_the_presented_secret() {
    let (mut worker, _room, _agent, dir) = protected_fixture();

    let accepted = call_with_capability(
        &mut worker,
        "receipt-1",
        "room.create",
        json!({"name":"kept"}),
        Some(CONTROL_TOKEN),
    );
    assert!(accepted.ok, "{accepted:?}");
    let rooms = worker.state.rooms().count();

    let (stored, _) = worker
        .dev_receipts
        .get("receipt-1")
        .expect("a mutation records a receipt");
    assert!(
        stored.capability.is_none(),
        "receipt retained the caller's raw capability"
    );

    // The receipt still replays a duplicate request instead of mutating twice.
    let replay = call_with_capability(
        &mut worker,
        "receipt-1",
        "room.create",
        json!({"name":"kept"}),
        Some(CONTROL_TOKEN),
    );
    assert!(replay.ok, "{replay:?}");
    assert_eq!(worker.state.rooms().count(), rooms);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn room_orchestrator_control_worker_adopts_the_armed_launch_capability() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    std::env::set_var(
        crate::bus::orchestrator_control::TOKEN_ENV_VAR,
        CONTROL_TOKEN,
    );
    crate::bus::orchestrator_control::arm_from_environment().unwrap();
    let (mut worker, _room, _agent, dir) = fixture();
    worker.adopt_launch_capability();

    let rejected = call(&mut worker, "adopt-1", "room.create", json!({"name":"x"}));
    let accepted = call_with_capability(
        &mut worker,
        "adopt-2",
        "room.create",
        json!({"name":"y"}),
        Some(CONTROL_TOKEN),
    );

    assert_eq!(rejected.error.unwrap().code, "capability_required");
    assert!(accepted.ok, "{accepted:?}");
    crate::bus::orchestrator_control::disarm_for_test();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn room_orchestrator_control_protected_instance_keeps_human_and_callback_paths_open() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let (mut worker, _room, agent, dir) = protected_fixture();
    let (events, _received) = mpsc::channel();
    let spool = dir.join("callbacks").join("launch");
    std::fs::create_dir_all(&spool).unwrap();

    // The Human at the TUI and the provider callbacks never present a capability.
    let created = worker.command(BusCommand::CreateRoom("human".into()), &events);
    let callbacks = worker.consume_callbacks(agent, &spool);

    assert!(created.is_ok(), "{created:?}");
    assert!(callbacks.is_ok(), "{callbacks:?}");

    // Assignment verification reads provider discovery, never the control boundary.
    std::env::set_var(
        crate::bus::orchestrator_control::TOKEN_ENV_VAR,
        CONTROL_TOKEN,
    );
    crate::bus::orchestrator_control::arm_from_environment().unwrap();
    let armed = crate::bus::trusted_assignment::verify_from_environment("frame");
    crate::bus::orchestrator_control::disarm_for_test();
    let unarmed = crate::bus::trusted_assignment::verify_from_environment("frame");

    assert_eq!(
        std::mem::discriminant(&armed),
        std::mem::discriminant(&unarmed),
        "assignment verify changed under --orchestrator-control"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn room_orchestrator_control_unprotected_instance_keeps_legacy_requests_working() {
    let (mut worker, _room, _agent, dir) = fixture();
    let before = worker.state.rooms().count();

    let legacy = call(
        &mut worker,
        "legacy-1",
        "room.create",
        json!({"name":"legacy"}),
    );
    let ignored_extra = call_with_capability(
        &mut worker,
        "legacy-2",
        "room.create",
        json!({"name":"legacy-two"}),
        Some(CONTROL_TOKEN),
    );

    assert!(legacy.ok, "{legacy:?}");
    assert!(ignored_extra.ok, "{ignored_extra:?}");
    assert_eq!(worker.state.rooms().count(), before + 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn room_orchestrator_core_cli_send_durably_queues_and_returns_message_request_identity() {
    let (mut worker, room, agent, dir) = fixture();
    let second = worker
        .state
        .create_agent(room, "claude1", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    worker
        .state
        .set_draft_text(room, "my unfinished draft")
        .unwrap();
    worker.state.set_draft_recipients(room, [second]).unwrap();
    let before = worker.state.room(room).unwrap().draft.clone();
    let result = call(
        &mut worker,
        "send-1",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"round trip"}),
    );
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["stage"], "queued");
    assert_eq!(worker.state.room(room).unwrap().draft, before);
    let requests = worker.state.requests().collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id, agent);
    assert_eq!(requests[0].phase, RequestPhase::Queued);
    assert_eq!(requests[0].prompt.text, "round trip");
    let result2 = call(
        &mut worker,
        "send-1",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"round trip"}),
    );
    assert_eq!(result2.result, result.result);
    assert_eq!(worker.state.requests().count(), 1);
    let conflict = call(
        &mut worker,
        "send-1",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"different"}),
    );
    assert_eq!(conflict.error.unwrap().code, "id_conflict");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_and_history_preserve_explicit_recipient_order() {
    let (mut worker, room, codex, dir) = fixture();
    let claude = worker
        .state
        .create_agent(room, "claude1", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    let cursor = worker
        .state
        .create_agent(room, "cursor1", Provider::Cursor, dir.clone(), None)
        .unwrap();

    let sent = call(
        &mut worker,
        "ordered-send",
        "message.send",
        json!({"room":"test","to":["cursor1","codex1","claude1","cursor1"],"text":"review"}),
    );
    assert!(sent.ok, "{sent:?}");
    let message = sent.result["message_id"].clone();
    let request_agents = sent.result["request_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| {
            worker
                .state
                .request(RequestId(id.as_u64().unwrap()))
                .unwrap()
                .agent_id
        })
        .collect::<Vec<_>>();
    assert_eq!(request_agents, [cursor, codex, claude]);

    let history = call(
        &mut worker,
        "ordered-history",
        "room.history",
        json!({"room":"test"}),
    );
    assert!(history.ok, "{history:?}");
    assert_eq!(history.result["messages"][0]["prompt"]["id"], message);
    let history_agents = history.result["messages"][0]["delivery"]["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["agent_id"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(history_agents, [cursor.0, codex.0, claude.0]);

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_selectors_confirmation_and_normal_mode_fail_closed() {
    let (mut worker, room, _agent, dir) = fixture();
    let other = worker.state.create_room("other").unwrap();
    let alien = worker
        .state
        .create_agent(other, "alien", Provider::Codex, dir.clone(), None)
        .unwrap();
    let bad = call(
        &mut worker,
        "bad",
        "message.send",
        json!({"room":"test","to":[alien.0.to_string()],"text":"no"}),
    );
    assert!(!bad.ok);
    assert_eq!(worker.state.requests().count(), 0);
    let delete = call(&mut worker, "delete", "room.delete", json!({"room":"test"}));
    assert_eq!(delete.error.unwrap().code, "confirmation_required");
    assert!(worker.state.room(room).is_some());
    worker.dev_enabled = false;
    let normal = call(&mut worker, "disabled", "state", json!({}));
    assert_eq!(normal.error.unwrap().code, "dev_disabled");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_status_explains_gate_without_claiming_delivery() {
    let (mut worker, _room, _agent, dir) = fixture();
    let receipt = call(
        &mut worker,
        "send",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"hello"}),
    );
    assert!(receipt.ok, "{receipt:?}");
    let status = call(
        &mut worker,
        "status",
        "message.status",
        json!({"message":receipt.result["message_id"].to_string()}),
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.result["complete"], false);
    assert_eq!(status.result["requests"][0]["stage"], "queued");
    assert_eq!(
        status.result["requests"][0]["reason"],
        "hook_setup_unconfirmed"
    );
    assert!(status.result["requests"][0]["reply"].is_null());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_human_abandon_idle_request_preserves_the_agent_and_queue() {
    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let sent = call(
        &mut worker,
        "send-wedged",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"wedged"}),
    );
    let request = RequestId(sent.result["request_ids"][0].as_u64().unwrap());
    let message = sent.result["message_id"].clone();
    worker
        .state
        .begin_submission(request, "launch", 10)
        .unwrap();
    worker
        .state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .unwrap();
    assert_eq!(
        worker.state.accept_callback(ProviderCallback {
            callback_id: "start".into(),
            sequence: 11,
            occurred_at_ms: 11,
            agent_id: agent,
            launch_id: "launch".into(),
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("wedged".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 12)
        .unwrap();

    let recovered = call(
        &mut worker,
        "recover-wedged",
        "request.recover",
        json!({"request":request.0.to_string(),"confirm":true}),
    );
    assert!(recovered.ok, "{recovered:?}");
    assert_eq!(recovered.result["stage"], "abandoned");
    assert_eq!(recovered.result["agent_id"], agent.0);
    assert_eq!(worker.state.agent(agent).unwrap().current_request, None);

    let status = call(
        &mut worker,
        "status-after-recovery",
        "message.status",
        json!({"message":message.to_string()}),
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.result["complete"], true);
    assert_eq!(status.result["requests"][0]["stage"], "abandoned");
    assert!(status.result["requests"][0]["reply"].is_null());

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_worker_read_uses_native_public_pane_selector() {
    struct InspectTarget;
    impl Transport for InspectTarget {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let Method::AgentGet(params) = method else {
                panic!("Expected identity inspection first")
            };
            assert_eq!(params.target, "w1:p2");
            Err(TransportError {
                code: None,
                message: "probe complete".into(),
                definitely_rejected: true,
            })
        }
    }
    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    worker.transport = Box::new(InspectTarget);
    let result = call(
        &mut worker,
        "read",
        "agent.read",
        json!({"agent":"codex1","source":"visible"}),
    );
    assert_eq!(result.error.unwrap().message, "probe complete");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_worker_visible_read_returns_viewport_and_correlation_facts() {
    struct VisibleInspect;
    impl Transport for VisibleInspect {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentGet(params) => {
                    assert_eq!(params.target, "w1:p2");
                    Ok(ResponseResult::AgentInfo {
                        agent: serde_json::from_value(json!({
                            "terminal_id": "term_internal",
                            "pane_id": "w1:p2",
                            "name": "bus-r1-a2",
                            "agent_status": "working",
                            "agent_session": {
                                "source": "herdr:codex",
                                "agent": "codex",
                                "kind": "id",
                                "value": "session"
                            },
                            "workspace_id": "w1",
                            "tab_id": "t1",
                            "focused": false,
                            "revision": 3
                        }))
                        .unwrap(),
                    })
                }
                Method::PaneGet(params) => {
                    assert_eq!(params.pane_id, "w1:p2");
                    Ok(ResponseResult::PaneInfo {
                        pane: owned_pane_info(Some(schema::PaneScrollInfo {
                            offset_from_bottom: 0,
                            max_offset_from_bottom: 12,
                            viewport_rows: 24,
                        })),
                    })
                }
                Method::AgentRead(params) => {
                    assert_eq!(params.target, "w1:p2");
                    assert_eq!(params.source, schema::ReadSource::Visible);
                    assert_eq!(params.lines, None);
                    assert_eq!(params.format, schema::ReadFormat::Text);
                    assert!(params.strip_ansi);
                    Ok(ResponseResult::PaneRead {
                        read: schema::PaneReadResult {
                            pane_id: "w1:p2".into(),
                            workspace_id: "w1".into(),
                            tab_id: "t1".into(),
                            source: schema::ReadSource::Visible,
                            format: schema::ReadFormat::Text,
                            text: "complete viewport\nrow two".into(),
                            revision: 9,
                            truncated: false,
                            viewport_rows: Some(24),
                            viewport_columns: Some(80),
                            requested_lines: None,
                            returned_lines: 24,
                            available_lines: None,
                            exhausted: None,
                        },
                    })
                }
                other => panic!("unexpected native method: {other:?}"),
            }
        }
    }
    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("session".into()),
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
            },
        )
        .unwrap();
    let sent = call(
        &mut worker,
        "send-visible",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"inspect me"}),
    );
    assert!(sent.ok, "{sent:?}");
    let request = RequestId(sent.result["request_ids"][0].as_u64().unwrap());
    worker.state.begin_submission(request, "launch", 4).unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Working, 5)
        .unwrap();
    worker.transport = Box::new(VisibleInspect);
    let before = crate::bus::io::now_ms();
    let result = call(
        &mut worker,
        "read-visible",
        "agent.read",
        json!({"agent":"codex1","source":"visible"}),
    );
    let after = crate::bus::io::now_ms();
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["agent_id"], agent.0);
    assert_eq!(result.result["name"], "codex1");
    assert_eq!(result.result["status"], "working");
    assert_eq!(result.result["current_request"], request.0);
    assert_eq!(result.result["runtime"]["launch_id"], "launch");
    assert_eq!(result.result["runtime"]["session_id"], "session");
    assert_eq!(result.result["runtime"]["pane_id"], "w1:p2");
    assert_eq!(result.result["runtime"]["terminal_id"], "term_internal");
    assert_eq!(result.result["capture"]["source"], "visible");
    assert_eq!(result.result["capture"]["truncated"], false);
    assert_eq!(result.result["capture"]["revision"], 9);
    assert!(result.result["capture"].get("lines").is_none());
    assert_eq!(result.result["capture"]["viewport"]["rows"], 24);
    assert_eq!(result.result["capture"]["viewport"]["columns"], 80);
    let captured = result.result["capture"]["at_ms"].as_u64().unwrap();
    assert!(
        captured >= before && captured <= after,
        "capture time {captured} outside {before}..={after}"
    );
    assert_eq!(result.result["text"], "complete viewport\nrow two");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

fn owned_pane_info(scroll: Option<schema::PaneScrollInfo>) -> schema::PaneInfo {
    let mut value = json!({
        "pane_id": "w1:p2",
        "terminal_id": "term_internal",
        "workspace_id": "w1",
        "tab_id": "t1",
        "focused": false,
        "agent_status": "working",
        "revision": 3
    });
    if let Some(scroll) = scroll {
        value["scroll"] = serde_json::to_value(scroll).unwrap();
    }
    serde_json::from_value(value).unwrap()
}

fn owned_agent_info(pane_id: &str, name: &str, session: Option<&str>) -> schema::AgentInfo {
    let mut value = json!({
        "terminal_id": "term_internal",
        "pane_id": pane_id,
        "name": name,
        "agent_status": "idle",
        "workspace_id": "w1",
        "tab_id": "t1",
        "focused": false,
        "revision": 1
    });
    if let Some(session) = session {
        value["agent_session"] = json!({
            "source": "herdr:codex",
            "agent": "codex",
            "kind": "id",
            "value": session
        });
    }
    serde_json::from_value(value).unwrap()
}

#[test]
fn room_orchestrator_core_worker_read_fails_closed_for_missing_or_stale_runtime_identity() {
    struct StaleInspect;
    impl Transport for StaleInspect {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentGet(_) => Ok(ResponseResult::AgentInfo {
                    agent: owned_agent_info("w1:other", "bus-r1-a2", Some("session")),
                }),
                Method::AgentRead(_) => panic!("stale identity must not read the pane"),
                other => panic!("unexpected native method: {other:?}"),
            }
        }
    }
    let (mut worker, _room, agent, dir) = fixture();
    let missing = call(
        &mut worker,
        "read-missing",
        "agent.read",
        json!({"agent":"codex1","source":"visible"}),
    );
    assert!(!missing.ok, "{missing:?}");
    assert_eq!(missing.error.unwrap().message, "Agent has no terminal");

    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("session".into()),
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
            },
        )
        .unwrap();
    worker.transport = Box::new(StaleInspect);
    let stale = call(
        &mut worker,
        "read-stale",
        "agent.read",
        json!({"agent":"codex1","source":"visible"}),
    );
    assert!(!stale.ok, "{stale:?}");
    assert_eq!(
        stale.error.unwrap().message,
        "Agent terminal identity changed; inspect the owned session"
    );
    let rejected = call(
        &mut worker,
        "read-detection-source",
        "agent.read",
        json!({"agent":"codex1","source":"detection"}),
    );
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(
        rejected.error.unwrap().message,
        "Source must be visible or recent"
    );
    let visible_lines = call(
        &mut worker,
        "read-visible-lines",
        "agent.read",
        json!({"agent":"codex1","source":"visible","lines":20}),
    );
    assert!(!visible_lines.ok, "{visible_lines:?}");
    assert_eq!(
        visible_lines.error.unwrap().message,
        "Visible reads return the complete viewport; omit lines"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_worker_recent_read_requires_explicit_lines_before_native_transport() {
    let (mut worker, _room, _agent, dir) = fixture();
    let result = call(&mut worker, "read", "agent.read", json!({"agent":"codex1"}));
    assert!(!result.ok, "{result:?}");
    assert_eq!(
        result.error.unwrap().message,
        "Recent reads require an explicit positive lines value"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_worker_recent_read_forwards_selected_lines_and_reports_range() {
    struct RecentRange;
    impl Transport for RecentRange {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentGet(params) => {
                    assert_eq!(params.target, "w1:p2");
                    Ok(ResponseResult::AgentInfo {
                        agent: owned_agent_info("w1:p2", "bus-r1-a2", Some("session")),
                    })
                }
                Method::AgentRead(params) => {
                    assert_eq!(params.source, schema::ReadSource::Recent);
                    assert_eq!(params.lines, Some(80));
                    Ok(ResponseResult::PaneRead {
                        read: schema::PaneReadResult {
                            pane_id: "w1:p2".into(),
                            workspace_id: "w1".into(),
                            tab_id: "t1".into(),
                            source: schema::ReadSource::Recent,
                            format: schema::ReadFormat::Text,
                            text: "line 79\nline 80".into(),
                            revision: 11,
                            truncated: true,
                            viewport_rows: None,
                            viewport_columns: None,
                            requested_lines: Some(80),
                            returned_lines: 80,
                            available_lines: Some(81),
                            exhausted: Some(false),
                        },
                    })
                }
                other => panic!("unexpected native method: {other:?}"),
            }
        }
    }
    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    worker.transport = Box::new(RecentRange);
    let result = call(
        &mut worker,
        "read-range",
        "agent.read",
        json!({"agent":"codex1","source":"recent","lines":80}),
    );
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["text"], "line 79\nline 80");
    assert_eq!(result.result["capture"]["source"], "recent");
    assert_eq!(result.result["capture"]["truncated"], true);
    assert_eq!(result.result["capture"]["revision"], 11);
    assert_eq!(result.result["capture"]["requested_lines"], 80);
    assert_eq!(result.result["capture"]["returned_lines"], 80);
    assert_eq!(result.result["capture"]["available_lines"], 81);
    assert_eq!(result.result["capture"]["exhausted"], false);
    assert!(result.result["capture"].get("native_max_lines").is_none());
    assert!(result.result["capture"].get("limit").is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_worker_recent_read_has_no_one_thousand_line_clamp() {
    struct UncappedRecent;
    impl Transport for UncappedRecent {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentGet(_) => Ok(ResponseResult::AgentInfo {
                    agent: owned_agent_info("w1:p2", "bus-r1-a2", Some("session")),
                }),
                Method::AgentRead(params) => {
                    assert_eq!(params.lines, Some(5000));
                    Ok(ResponseResult::PaneRead {
                        read: schema::PaneReadResult {
                            pane_id: "w1:p2".into(),
                            workspace_id: "w1".into(),
                            tab_id: "t1".into(),
                            source: schema::ReadSource::Recent,
                            format: schema::ReadFormat::Text,
                            text: (0..1200)
                                .map(|index| format!("row-{index:04}"))
                                .collect::<Vec<_>>()
                                .join("\n"),
                            revision: 2,
                            truncated: false,
                            viewport_rows: None,
                            viewport_columns: None,
                            requested_lines: Some(5000),
                            returned_lines: 1200,
                            available_lines: Some(1200),
                            exhausted: Some(true),
                        },
                    })
                }
                other => panic!("unexpected native method: {other:?}"),
            }
        }
    }
    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    worker.transport = Box::new(UncappedRecent);
    let result = call(
        &mut worker,
        "read-uncapped",
        "agent.read",
        json!({"agent":"codex1","source":"recent","lines":5000}),
    );
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["capture"]["requested_lines"], 5000);
    assert_eq!(result.result["capture"]["returned_lines"], 1200);
    assert_eq!(result.result["capture"]["truncated"], false);
    assert_eq!(result.result["capture"]["exhausted"], true);
    assert!(result.result["capture"].get("limit").is_none());
    assert!(result.result["capture"].get("native_max_lines").is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn agent_approve_once_worker_rejects_stale_launch_before_native_then_persists_and_rejects_replay() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct PermissionTransport {
        approvals: Arc<AtomicUsize>,
    }
    impl Transport for PermissionTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentPermissionObserve(params) => {
                    assert_eq!(params.target, "w1:p2");
                    Ok(ResponseResult::AgentPermission {
                        observation: schema::AgentPermissionObservation {
                            terminal_id: "term_internal".into(),
                            pane_id: "w1:p2".into(),
                            session_id: "session".into(),
                            content_revision: 22,
                            prompt_digest: "prompt-digest".into(),
                            prompt_text: "Allow read-only command: rg --files".into(),
                            eligibility: schema::PermissionEligibility::Allowlisted {
                                action: schema::SafePermissionAction::ReadOnlyInspection,
                                root: "/repo".into(),
                            },
                            allowed_responses: vec![schema::ApprovedPermissionResponse::AllowOnce],
                        },
                    })
                }
                Method::AgentApproveOnce(params) => {
                    assert_eq!(params.expected_terminal_id, "term_internal");
                    assert_eq!(params.expected_content_revision, 22);
                    self.approvals.fetch_add(1, Ordering::SeqCst);
                    Ok(ResponseResult::AgentApprovedOnce {
                        approval: schema::AgentApproveOnceResult {
                            written: true,
                            reason: None,
                            observation: schema::AgentPermissionObservation {
                                terminal_id: "term_internal".into(),
                                pane_id: "w1:p2".into(),
                                session_id: "session".into(),
                                content_revision: 22,
                                prompt_digest: "prompt-digest".into(),
                                prompt_text: "Allow read-only command: rg --files".into(),
                                eligibility: schema::PermissionEligibility::Allowlisted {
                                    action: schema::SafePermissionAction::ReadOnlyInspection,
                                    root: "/repo".into(),
                                },
                                allowed_responses: vec![
                                    schema::ApprovedPermissionResponse::AllowOnce,
                                ],
                            },
                        },
                    })
                }
                other => panic!("unexpected native method: {other:?}"),
            }
        }
    }

    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("term_internal".into()),
                pane_id: Some("w1:p2".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    let sent = call(
        &mut worker,
        "send-permission",
        "message.send",
        json!({
            "room":"test","to":["codex1"],"text":"inspect"
        }),
    );
    let request = RequestId(sent.result["request_ids"][0].as_u64().unwrap());
    worker.state.begin_submission(request, "launch", 1).unwrap();
    worker
        .state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .unwrap();
    let approvals = Arc::new(AtomicUsize::new(0));
    worker.transport = Box::new(PermissionTransport {
        approvals: approvals.clone(),
    });
    let observed = call(
        &mut worker,
        "observe-permission",
        "agent.permission.observe",
        json!({"agent":"codex1"}),
    );
    assert!(observed.ok, "{observed:?}");
    let fingerprint = observed.result["fingerprint"].as_str().unwrap().to_owned();
    assert_eq!(observed.result["launch_id"], "launch");
    assert_eq!(observed.result["current_request"], request.0);
    assert_eq!(observed.result["provider_turn"], "turn-1");
    let mut state = worker.state.clone();
    state
        .orchestrator_state_mut()
        .grant(
            _room,
            crate::bus::orchestrator::ParticipantId::Human,
            crate::bus::orchestrator::ParticipantId::Orchestrator,
            crate::bus::orchestrator::Capability::ApprovePermissionOnce,
        )
        .unwrap();
    worker.save(state).unwrap();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("replacement-launch".into()),
                terminal_id: Some("term_internal".into()),
                pane_id: Some("w1:p2".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    let stale = worker.approve_permission_once(
        crate::bus::orchestrator::ParticipantId::Orchestrator,
        Some(agent),
        crate::bus::orchestrator::ExactPermissionGrant {
            fingerprint: fingerprint.clone(),
            response: crate::bus::orchestrator::ApprovedPermissionResponse::AllowOnce,
        },
    );
    assert!(stale.is_err());
    assert_eq!(approvals.load(Ordering::SeqCst), 0);
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("term_internal".into()),
                pane_id: Some("w1:p2".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    let approved = worker
        .approve_permission_once(
            crate::bus::orchestrator::ParticipantId::Orchestrator,
            Some(agent),
            crate::bus::orchestrator::ExactPermissionGrant {
                fingerprint: fingerprint.clone(),
                response: crate::bus::orchestrator::ApprovedPermissionResponse::AllowOnce,
            },
        )
        .unwrap();
    assert_eq!(approved["written"], true);
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
    let replay = call(
        &mut worker,
        "replay-permission",
        "agent.permission.approve_once",
        json!({
            "agent":"codex1","fingerprint":fingerprint,"response":"allow-once"
        }),
    );
    assert!(!replay.ok);
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_history_retains_twenty_ordered_messages_and_their_own_final_replies() {
    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut messages = Vec::new();
    for i in 0..20 {
        let receipt = call(
            &mut worker,
            &format!("send-{i}"),
            "message.send",
            json!({"room":"test","to":["codex1"],"text":format!("message-{i}")}),
        );
        assert!(receipt.ok, "{receipt:?}");
        messages.push(receipt.result["message_id"].clone());
        let request = RequestId(receipt.result["request_ids"][0].as_u64().unwrap());
        let seq = (i + 1) * 10;
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, seq)
            .unwrap();
        worker
            .state
            .begin_submission(request, "launch", seq)
            .unwrap();
        worker
            .state
            .record_submission(
                request,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("session".into()),
                    provider_turn_id: Some(format!("turn-{i}")),
                },
            )
            .unwrap();
        assert_eq!(
            worker.state.accept_callback(ProviderCallback {
                callback_id: format!("start-{i}"),
                sequence: seq + 1,
                occurred_at_ms: seq + 1,
                agent_id: agent,
                launch_id: "launch".into(),
                provider_session_id: Some("session".into()),
                provider_turn_id: Some(format!("turn-{i}")),
                provider_prompt_id: None,
                prompt_payload: Some(format!("message-{i}")),
                kind: CallbackEventKind::PromptStarted
            }),
            CallbackDisposition::AcceptedBinding
        );
        worker.state.accept_callback(ProviderCallback::final_event(
            &format!("final-{i}"),
            seq + 2,
            agent,
            "launch",
            "session",
            &format!("turn-{i}"),
            &format!("message-{i}"),
            &format!("reply-{i}"),
        ));
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, seq + 3)
            .unwrap();
    }
    worker.save(worker.state.clone()).unwrap();
    let result = call(
        &mut worker,
        "history",
        "room.history",
        json!({"room":"test"}),
    );
    let history = result.result["messages"].as_array().unwrap();
    assert_eq!(history.len(), 20);
    for (i, message) in history.iter().enumerate() {
        assert_eq!(message["prompt"]["id"], messages[i]);
        assert_eq!(message["delivery"]["complete"], true);
        assert_eq!(
            message["delivery"]["requests"][0]["reply"]["text"],
            format!("reply-{i}")
        );
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_recipient_sets_and_failed_sends_never_modify_draft_or_broadcast_implicitly() {
    let (mut worker, room, first, dir) = fixture();
    let mut ids = vec![first];
    for name in ["second", "third", "fourth"] {
        ids.push(
            worker
                .state
                .create_agent(room, name, Provider::Codex, dir.clone(), None)
                .unwrap(),
        );
    }
    worker.state.set_draft_text(room, "keep me").unwrap();
    let draft = worker.state.room(room).unwrap().draft.clone();
    for size in 1..=4 {
        let to = if size == 4 {
            json!(["all"])
        } else {
            json!(ids[..size]
                .iter()
                .map(|id| id.0.to_string())
                .collect::<Vec<_>>())
        };
        let sent = call(
            &mut worker,
            &format!("set-{size}"),
            "message.send",
            json!({"room":"test","to":to,"text":"check"}),
        );
        assert!(sent.ok, "{sent:?}");
        let actual = sent.result["request_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                worker
                    .state
                    .request(RequestId(v.as_u64().unwrap()))
                    .unwrap()
                    .agent_id
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, ids[..size].iter().copied().collect());
        assert_eq!(worker.state.room(room).unwrap().draft, draft);
    }
    let count = worker.state.requests().count();
    for params in [
        json!({"room":"test","to":[],"text":"no"}),
        json!({"room":"test","text":"no"}),
        json!({"room":"test","to":["codex1"],"text":"no","files":["/does-not-exist/bus-file"]}),
    ] {
        assert!(
            !call(
                &mut worker,
                &format!("invalid-{params}"),
                "message.send",
                params
            )
            .ok
        );
        assert_eq!(worker.state.room(room).unwrap().draft, draft);
        assert_eq!(worker.state.requests().count(), count);
    }
    worker.storage_failed = true;
    assert_eq!(
        call(
            &mut worker,
            "storage",
            "message.send",
            json!({"room":"test","to":["all"],"text":"no"})
        )
        .error
        .unwrap()
        .code,
        "storage_unavailable"
    );
    assert!(call(&mut worker, "diag", "diagnostics", json!({})).ok);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
