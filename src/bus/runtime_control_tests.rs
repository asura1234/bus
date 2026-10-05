use super::*;
use crate::bus::transport::TransportError;
use serde_json::json;

struct NoTransport;
impl Transport for NoTransport {
    fn request(&mut self, _method: Method) -> Result<ResponseResult, TransportError> {
        panic!("Domain-only command unexpectedly reached native transport")
    }
}

static NEXT_FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn fixture() -> (Worker, RoomId, AgentId, PathBuf) {
    // Parallel tests can share a nanosecond; the counter keeps each coordinator lock unique.
    let dir = std::env::temp_dir().join(format!(
        "bus-control-domain-{}-{}-{}",
        std::process::id(),
        super::super::super::io::now_ns(),
        NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
    })
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
fn dev_room_notes_replace_and_clear_the_room_notes() {
    let (mut worker, room, _agent, dir) = fixture();
    let set = call(
        &mut worker,
        "notes-1",
        "room.notes",
        json!({"room":"test","text":"Goal\nNon-goals"}),
    );
    assert!(set.ok, "{set:?}");
    assert_eq!(worker.state.room(room).unwrap().notes, "Goal\nNon-goals");
    let state = call(&mut worker, "state-1", "state", json!({}));
    // MASTER is listed first; the fixture room follows.
    assert_eq!(state.result["rooms"][1]["notes"], "Goal\nNon-goals");
    let cleared = call(
        &mut worker,
        "notes-2",
        "room.notes",
        json!({"room":room.0.to_string(),"text":""}),
    );
    assert!(cleared.ok, "{cleared:?}");
    assert_eq!(worker.state.room(room).unwrap().notes, "");
    let missing = call(&mut worker, "notes-3", "room.notes", json!({"room":"test"}));
    assert!(!missing.ok);
    let unknown = call(
        &mut worker,
        "notes-4",
        "room.notes",
        json!({"room":"nope","text":"x"}),
    );
    assert!(!unknown.ok);
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
    let stale = worker.approve_permission_once(Some(agent), fingerprint.clone());
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
        .approve_permission_once(Some(agent), fingerprint.clone())
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
    let saved = worker.store.load().unwrap().unwrap();
    assert!(saved.permission_fingerprint_consumed(&fingerprint));
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

fn error_message(response: &Response) -> String {
    response
        .error
        .as_ref()
        .map(|error| error.message.clone())
        .unwrap_or_default()
}

#[test]
fn dev_master_room_is_selectable_listed_and_fixed() {
    let (mut worker, room, _agent, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let state = call(&mut worker, "state-master", "state", json!({}));
    assert_eq!(state.result["master_room"], json!(master));
    assert_eq!(state.result["rooms"][0]["kind"], "master");
    assert_eq!(state.result["rooms"][1]["kind"], "work");
    assert_eq!(state.result["rooms"][1]["id"], json!(room));

    for (id, selector) in [("focus-a", "master"), ("focus-b", "MASTER")] {
        let notes = call(
            &mut worker,
            id,
            "room.notes",
            json!({"room": selector, "text": "orchestrators"}),
        );
        assert!(notes.ok, "{notes:?}");
    }
    assert_eq!(worker.state.room(master).unwrap().notes, "orchestrators");

    let rename = call(
        &mut worker,
        "rename-master",
        "room.rename",
        json!({"room":"master","name":"other"}),
    );
    assert!(!rename.ok);
    assert_eq!(
        error_message(&rename),
        "The MASTER room cannot be renamed or deleted"
    );
    let delete = call(
        &mut worker,
        "delete-master",
        "room.delete",
        json!({"room":"master","confirm":true}),
    );
    assert!(!delete.ok);
    assert_eq!(
        error_message(&delete),
        "The MASTER room cannot be renamed or deleted"
    );
    assert!(worker.state.master_room().is_some());
    let reserved = call(
        &mut worker,
        "create-master",
        "room.create",
        json!({"name":"Master"}),
    );
    assert!(
        error_message(&reserved).contains("reserved"),
        "{reserved:?}"
    );

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_agent_orchestrate_assigns_rejects_a_second_orchestrator_and_unassigns() {
    let (mut worker, room, agent, dir) = fixture();
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
    let second = worker
        .state
        .create_agent(master, "codex-orch", Provider::Codex, dir.clone(), None)
        .unwrap();

    let assigned = call(
        &mut worker,
        "orch-1",
        "agent.orchestrate",
        json!({"agent":"claude-orch","room":"test"}),
    );
    assert!(assigned.ok, "{assigned:?}");
    assert_eq!(worker.state.agent(first).unwrap().orchestrates, Some(room));
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

    let taken = call(
        &mut worker,
        "orch-2",
        "agent.orchestrate",
        json!({"agent":"codex-orch","room":"test"}),
    );
    assert!(!taken.ok);
    assert!(
        error_message(&taken).contains("already orchestrated by agent"),
        "{taken:?}"
    );
    assert_eq!(worker.state.agent(second).unwrap().orchestrates, None);

    let outside = call(
        &mut worker,
        "orch-3",
        "agent.orchestrate",
        json!({"agent":"codex1","room":"test"}),
    );
    assert!(
        error_message(&outside).contains("not in the MASTER room"),
        "{outside:?}"
    );
    assert_eq!(worker.state.agent(agent).unwrap().orchestrates, None);

    let cleared = call(
        &mut worker,
        "orch-4",
        "agent.orchestrate",
        json!({"agent":"claude-orch","room":null}),
    );
    assert!(cleared.ok, "{cleared:?}");
    assert_eq!(worker.state.agent(first).unwrap().orchestrates, None);

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
            "room":"test","name":"orch","provider":"codex",
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
fn dev_agent_rename_and_room_seen_match_tui_commands() {
    let (mut worker, room, agent, dir) = fixture();
    let renamed = call(
        &mut worker,
        "rename-1",
        "agent.rename",
        json!({"agent":"codex1","name":"Reviewer"}),
    );
    assert!(renamed.ok, "{renamed:?}");
    assert_eq!(worker.state.agent(agent).unwrap().name, "Reviewer");
    let blank = call(
        &mut worker,
        "rename-2",
        "agent.rename",
        json!({"agent":"Reviewer","name":"  "}),
    );
    assert_eq!(blank.error.unwrap().code, "command_failed");

    // Replies arrive through the full callback path; inject the count directly.
    let mut saved = serde_json::to_value(&worker.state).unwrap();
    saved["rooms"][room.0.to_string()]["unread_count"] = json!(3);
    worker.state = serde_json::from_value(saved).unwrap();
    let state = call(&mut worker, "state-1", "state", json!({}));
    let reported = state.result["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(room))
        .unwrap();
    assert_eq!(reported["unread_count"], 3);
    assert_eq!(state.result["visible_room"], serde_json::Value::Null);

    let seen = call(&mut worker, "seen-1", "room.seen", json!({"room":"test"}));
    assert!(seen.ok, "{seen:?}");
    assert_eq!(worker.state.room(room).unwrap().unread_count, 0);
    // Marking seen is not navigation: the human's view is unchanged.
    assert_eq!(worker.state.visible_room(), None);

    worker.state.select_room(room).unwrap();
    let state = call(&mut worker, "state-2", "state", json!({}));
    assert_eq!(state.result["visible_room"], json!(room));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_as_records_agent_author_and_skips_it_for_all() {
    let (mut worker, room, codex, dir) = fixture();
    let claude = worker
        .state
        .create_agent(room, "claude1", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    let other_room = worker.state.create_room("other").unwrap();
    worker
        .state
        .create_agent(other_room, "outsider", Provider::Codex, dir.clone(), None)
        .unwrap();

    let sent = call(
        &mut worker,
        "as-all",
        "message.send",
        json!({"room":"test","to":["all"],"text":"done","as":"codex1"}),
    );
    assert!(sent.ok, "{sent:?}");
    let requests = worker.state.requests().collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id, claude);
    assert_eq!(requests[0].prompt.author, Author::Agent(codex));
    let status = call(
        &mut worker,
        "as-history",
        "room.history",
        json!({"room":"test"}),
    );
    assert_eq!(
        status.result["messages"][0]["prompt"]["author"],
        json!({"agent": codex})
    );

    let human = call(
        &mut worker,
        "as-default",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"hi"}),
    );
    assert!(human.ok, "{human:?}");
    let message = PromptId(human.result["message_id"].as_u64().unwrap());
    assert!(worker
        .state
        .requests()
        .filter(|r| r.prompt.id == message)
        .all(|r| r.prompt.author == Author::Human));

    for (id, to, author, expected) in [
        (
            "as-self",
            json!(["codex1"]),
            "codex1",
            "cannot send a message to itself",
        ),
        (
            "as-self-mixed",
            json!(["claude1", "codex1"]),
            "codex1",
            "cannot send a message to itself",
        ),
        (
            "as-outsider",
            json!(["claude1"]),
            "outsider",
            "No matching room or agent",
        ),
    ] {
        let failed = call(
            &mut worker,
            id,
            "message.send",
            json!({"room":"test","to":to,"text":"x","as":author}),
        );
        let message = failed.error.unwrap().message;
        assert!(message.contains(expected), "{id}: {message}");
    }

    worker.state.prepare_delete_agent(codex).unwrap();
    let deleting = call(
        &mut worker,
        "as-deleting",
        "message.send",
        json!({"room":"test","to":["claude1"],"text":"x","as":"codex1"}),
    );
    assert!(deleting.error.unwrap().message.contains("being deleted"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_as_the_only_room_agent_to_all_has_no_recipients() {
    let (mut worker, _room, _agent, dir) = fixture();
    let sent = call(
        &mut worker,
        "as-alone",
        "message.send",
        json!({"room":"test","to":["all"],"text":"x","as":"codex1"}),
    );
    assert!(!sent.ok, "{sent:?}");
    assert_eq!(worker.state.requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_as_allows_only_the_rooms_own_master_orchestrator_from_outside() {
    let (mut worker, room, codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let other = worker.state.create_room("other").unwrap();
    let orchestrator = worker
        .state
        .create_agent(
            master,
            "claude-orch",
            Provider::ClaudeCode,
            dir.clone(),
            None,
        )
        .unwrap();
    worker
        .state
        .set_agent_orchestrates(orchestrator, Some(room))
        .unwrap();
    let elsewhere = worker
        .state
        .create_agent(master, "codex-orch", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker
        .state
        .set_agent_orchestrates(elsewhere, Some(other))
        .unwrap();
    worker
        .state
        .create_agent(master, "idle-orch", Provider::Codex, dir.clone(), None)
        .unwrap();

    let sent = call(
        &mut worker,
        "orch-as",
        "message.send",
        json!({"room":"test","to":["all"],"text":"plan","as":"claude-orch"}),
    );
    assert!(sent.ok, "{sent:?}");
    let requests = worker.state.requests().collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id, codex);
    assert_eq!(requests[0].prompt.author, Author::Agent(orchestrator));

    // By ID too, and an orchestrator can address the MASTER room it lives in.
    let by_id = call(
        &mut worker,
        "orch-as-id",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"x","as":orchestrator.0.to_string()}),
    );
    assert!(by_id.ok, "{by_id:?}");
    let in_master = call(
        &mut worker,
        "orch-in-master",
        "message.send",
        json!({"room":"master","to":["idle-orch"],"text":"x","as":"claude-orch"}),
    );
    assert!(in_master.ok, "{in_master:?}");

    for (id, author) in [("orch-other", "codex-orch"), ("orch-none", "idle-orch")] {
        let rejected = call(
            &mut worker,
            id,
            "message.send",
            json!({"room":"test","to":["codex1"],"text":"x","as":author}),
        );
        assert!(
            error_message(&rejected).contains("in this room or its orchestrator"),
            "{id}: {rejected:?}"
        );
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_room_sound_toggles_each_room_and_state_reports_it() {
    let (mut worker, room, _agent, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let state = call(&mut worker, "sound-state-1", "state", json!({}));
    assert_eq!(state.result["rooms"][0]["sound"], true);
    assert_eq!(state.result["rooms"][1]["sound"], false);

    let on = call(
        &mut worker,
        "sound-on",
        "room.sound",
        json!({"room":"test","on":true}),
    );
    assert!(on.ok, "{on:?}");
    let off = call(
        &mut worker,
        "sound-off",
        "room.sound",
        json!({"room":"MASTER","on":false}),
    );
    assert!(off.ok, "{off:?}");
    assert!(worker.state.room(room).unwrap().sound_enabled());
    assert!(!worker.state.room(master).unwrap().sound_enabled());
    let saved = worker.store.load().unwrap().unwrap();
    assert!(saved.room(room).unwrap().sound_enabled());

    let state = call(&mut worker, "sound-state-2", "state", json!({}));
    assert_eq!(state.result["rooms"][0]["sound"], false);
    assert_eq!(state.result["rooms"][1]["sound"], true);
    let invalid = call(
        &mut worker,
        "sound-bad",
        "room.sound",
        json!({"room":"test","on":"yes"}),
    );
    assert!(!invalid.ok);

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
