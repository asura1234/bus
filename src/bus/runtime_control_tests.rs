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
    let master = call(
        &mut worker,
        "notes-5",
        "room.notes",
        json!({"room":"MASTER","text":"x"}),
    );
    assert!(!master.ok, "{master:?}");
    assert_eq!(
        master.error.unwrap().message,
        "The MASTER room has no notes"
    );
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
fn dev_status_lists_the_message_files_as_absolute_paths() {
    let (mut worker, _room, _agent, dir) = fixture();
    let image = dir.join("shot.png");
    std::fs::write(&image, b"png").unwrap();
    let receipt = call(
        &mut worker,
        "send",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"see","files":[image]}),
    );
    assert!(receipt.ok, "{receipt:?}");
    let status = call(
        &mut worker,
        "status",
        "message.status",
        json!({"message":receipt.result["message_id"].to_string()}),
    );
    assert!(status.ok, "{status:?}");
    let files = status.result["files"].as_array().expect("files array");
    assert_eq!(files.len(), 1);
    let path = std::path::Path::new(files[0].as_str().unwrap());
    assert!(path.is_absolute());
    assert_eq!(path.file_name().unwrap(), "shot.png");
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

fn owned_pane_info() -> schema::PaneInfo {
    serde_json::from_value(json!({
        "pane_id": "w1:p2",
        "terminal_id": "term_internal",
        "workspace_id": "w1",
        "tab_id": "t1",
        "focused": false,
        "agent_status": "working",
        "revision": 3
    }))
    .unwrap()
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
fn agent_dialog_choose_is_identity_bound_single_use_and_reports_the_outcome() {
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Native {
        shown: bool,
        written: bool,
        question: bool,
        choices: Vec<schema::AgentDialogChooseParams>,
        answers: Vec<schema::AgentDialogAnswerParams>,
    }
    struct DialogTransport(Arc<Mutex<Native>>);
    fn observation(shown: bool) -> schema::AgentDialogObservation {
        schema::AgentDialogObservation {
            pending_question: false,
            terminal_id: "term_internal".into(),
            pane_id: "w1:p2".into(),
            session_id: None,
            content_revision: 22,
            dialog: shown.then(|| schema::AgentDialog {
                kind: schema::AgentDialogKind::Choice,
                text: "Do you trust the contents of this directory?".into(),
                options: vec![
                    schema::AgentDialogOption {
                        number: 1,
                        label: "Yes, continue".into(),
                        selected: true,
                    },
                    schema::AgentDialogOption {
                        number: 2,
                        label: "No, quit".into(),
                        selected: false,
                    },
                ],
                hint: Some("Press enter to continue".into()),
                id: "dialog-id".into(),
                digest: "dialog-digest".into(),
            }),
        }
    }
    impl Transport for DialogTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let mut native = self.0.lock().unwrap();
            match method {
                Method::AgentDialogObserve(params) => {
                    assert_eq!(params.target, "w1:p2");
                    let mut observed = observation(native.shown);
                    if native.question {
                        if let Some(dialog) = &mut observed.dialog {
                            dialog.kind = schema::AgentDialogKind::Question;
                            dialog.options.clear();
                        }
                    }
                    Ok(ResponseResult::AgentDialog {
                        observation: observed,
                    })
                }
                Method::AgentDialogChoose(params) => {
                    native.choices.push(params);
                    let written = native.written;
                    native.shown = !written;
                    Ok(ResponseResult::AgentDialogChosen {
                        choice: schema::AgentDialogChooseResult {
                            written,
                            reason: (!written).then(|| "stale_or_changed_dialog".into()),
                            keys: if written {
                                vec!["down".into(), "enter".into()]
                            } else {
                                vec![]
                            },
                            observation: observation(native.shown),
                        },
                    })
                }
                Method::AgentDialogAnswer(params) => {
                    let keys = if params.skip {
                        vec!["ctrl+]".into()]
                    } else {
                        vec!["enter".into()]
                    };
                    native.answers.push(params);
                    let written = native.written;
                    native.shown = !written;
                    Ok(ResponseResult::AgentDialogChosen {
                        choice: schema::AgentDialogChooseResult {
                            written,
                            reason: (!written).then(|| "stale_or_changed_dialog".into()),
                            keys: if written { keys } else { vec![] },
                            observation: observation(native.shown),
                        },
                    })
                }
                other => panic!("unexpected native method: {other:?}"),
            }
        }
    }

    let (mut worker, _room, agent, dir) = fixture();
    // Still launching: the trust dialog comes before any session is bound.
    let identity = |launch: &str| AgentRuntimeIdentity {
        launch_id: Some(launch.into()),
        terminal_id: Some("term_internal".into()),
        pane_id: Some("w1:p2".into()),
        session_id: None,
    };
    worker
        .state
        .set_agent_runtime_identity(agent, identity("launch"))
        .unwrap();
    let relaunch = |worker: &mut Worker, launch: &str| {
        let mut state = worker.state.clone();
        state
            .set_agent_runtime_identity(agent, identity(launch))
            .unwrap();
        worker.save(state).unwrap();
    };
    let native = Arc::new(Mutex::new(Native::default()));
    worker.transport = Box::new(DialogTransport(native.clone()));

    let none = call(
        &mut worker,
        "observe-none",
        "agent.dialog.observe",
        json!({"agent":"codex1"}),
    );
    assert!(none.ok, "{none:?}");
    assert_eq!(none.result["dialog"], Value::Null);
    assert_eq!(none.result["fingerprint"], Value::Null);

    native.lock().unwrap().shown = true;
    let observed = call(
        &mut worker,
        "observe",
        "agent.dialog.observe",
        json!({"agent":"codex1"}),
    );
    assert_eq!(observed.result["dialog"]["options"][1]["label"], "No, quit");
    let fingerprint = observed.result["fingerprint"].as_str().unwrap().to_owned();
    // Mutations replay by request ID, so each attempt needs its own.
    let attempts = std::cell::Cell::new(0);
    let choose = |worker: &mut Worker, option: &str, fingerprint: &str| {
        attempts.set(attempts.get() + 1);
        call(
            worker,
            &format!("choose-{}", attempts.get()),
            "agent.dialog.choose",
            json!({"agent":"codex1","option":option,"fingerprint":fingerprint}),
        )
    };

    // A relaunched agent, a missing option or a forged fingerprint sends nothing.
    relaunch(&mut worker, "replacement-launch");
    assert!(!choose(&mut worker, "2", &fingerprint).ok);
    relaunch(&mut worker, "launch");
    assert!(!choose(&mut worker, "3", &fingerprint).ok);
    assert!(!choose(&mut worker, "1", &fingerprint.replace("d1.", "d1.x")).ok);
    assert!(native.lock().unwrap().choices.is_empty());

    // The native layer rejects a dialog that changed; the fingerprint is spent.
    let stale = choose(&mut worker, "2", &fingerprint);
    assert!(!stale.ok);
    let message = stale.error.unwrap().message;
    assert!(message.contains("No keys were sent"), "{message}");
    assert_eq!(native.lock().unwrap().choices.len(), 1);
    assert!(!choose(&mut worker, "2", &fingerprint).ok);
    assert_eq!(native.lock().unwrap().choices.len(), 1);

    let fresh = call(
        &mut worker,
        "observe-again",
        "agent.dialog.observe",
        json!({"agent":"codex1"}),
    );
    let fresh = fresh.result["fingerprint"].as_str().unwrap().to_owned();
    assert_ne!(fresh, fingerprint);
    native.lock().unwrap().written = true;
    let chosen = choose(&mut worker, "2", &fresh);
    assert!(chosen.ok, "{chosen:?}");
    assert_eq!(chosen.result["outcome"], "closed");
    assert_eq!(chosen.result["keys"], json!(["down", "enter"]));
    let sent = native.lock().unwrap().choices[1].clone();
    assert_eq!(sent.option, 2);
    assert_eq!(sent.expected_dialog_digest, "dialog-digest");
    assert_eq!(sent.expected_session_id, None);

    assert!(!choose(&mut worker, "2", &fresh).ok);
    assert_eq!(native.lock().unwrap().choices.len(), 2);
    let saved = worker.store.load().unwrap().unwrap();
    assert!(saved.dialog_fingerprint_consumed(&fresh));
    assert!(worker
        .answer_dialog(agent, Some("token"), false, &fresh)
        .is_err());
    native.lock().unwrap().question = true;
    native.lock().unwrap().shown = true;
    let fingerprint = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    for (text, skip) in [(None, false), (Some("token"), true), (Some(" "), false)] {
        assert!(worker
            .answer_dialog(agent, text, skip, &fingerprint)
            .is_err());
    }
    relaunch(&mut worker, "other-launch");
    assert!(worker
        .answer_dialog(agent, Some("token"), false, &fingerprint)
        .is_err());
    relaunch(&mut worker, "launch");
    assert!(native.lock().unwrap().answers.is_empty());
    native.lock().unwrap().written = false;
    assert!(worker
        .answer_dialog(agent, Some("token"), false, &fingerprint)
        .is_err());
    assert_eq!(native.lock().unwrap().answers.len(), 1);
    assert!(worker
        .answer_dialog(agent, Some("token"), false, &fingerprint)
        .is_err());
    assert_eq!(native.lock().unwrap().answers.len(), 1);
    native.lock().unwrap().written = true;
    let fresh = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let answered = worker
        .answer_dialog(agent, Some("token"), false, &fresh)
        .unwrap();
    assert_eq!(answered["outcome"], "closed");
    assert!(answered.get("option").is_none());
    assert!(worker.state.agent(agent).unwrap().dialog_answer.is_none());
    // A real newly reported question clears any previously recorded option.
    worker
        .state
        .set_dialog_notice(agent, Some("question".into()))
        .unwrap();
    native.lock().unwrap().shown = true;
    let fresh = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let skipped = worker.answer_dialog(agent, None, true, &fresh).unwrap();
    assert_eq!(skipped["outcome"], "closed");
    assert_eq!(skipped["keys"], json!(["ctrl+]"]));
    assert!(worker.state.agent(agent).unwrap().dialog_answer.is_none());
    assert!(worker.answer_dialog(agent, None, true, &fresh).is_err());
    assert!(worker
        .store
        .load()
        .unwrap()
        .unwrap()
        .dialog_fingerprint_consumed(&fresh));
    let answers = &native.lock().unwrap().answers;
    assert_eq!(answers[1].text.as_deref(), Some("token"));
    assert_eq!(answers[2].text, None);
    assert!(answers[2].skip);
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
    worker.storage_pause = Some(StoragePause::NeedsRepair);
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

    for (id, selector, on) in [("focus-a", "master", false), ("focus-b", "MASTER", true)] {
        let sound = call(
            &mut worker,
            id,
            "room.sound",
            json!({"room": selector, "on": on}),
        );
        assert!(sound.ok, "{sound:?}");
        assert_eq!(worker.state.room(master).unwrap().sound, Some(on));
    }

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
            crate::bus::orchestrator::CODEX_ORCHESTRATOR_REFUSED,
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
    assert!(
        error_message(&sent).starts_with("No recipients besides the author"),
        "{sent:?}"
    );
    assert_eq!(worker.state.requests().count(), 0);
    let empty = worker.state.create_room("empty").unwrap();
    let nobody = call(
        &mut worker,
        "all-empty",
        "message.send",
        json!({"room":"empty","to":["all"],"text":"x"}),
    );
    assert_eq!(
        error_message(&nobody),
        "No recipients: the room has no agents"
    );

    // Deleting the visible room moves the view where the UI goes: MASTER.
    worker.state.select_room(empty).unwrap();
    let deleted = call(
        &mut worker,
        "delete-visible",
        "room.delete",
        json!({"room":"empty","confirm":true}),
    );
    assert!(deleted.ok, "{deleted:?}");
    let state = call(&mut worker, "state-after-delete", "state", json!({}));
    assert_eq!(state.result["visible_room"], state.result["master_room"]);
    assert!(!state.result["visible_room"].is_null());
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
    worker.state.bind_orchestrator(orchestrator, room).unwrap();
    let elsewhere = worker
        .state
        .create_agent(master, "codex-orch", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker.state.bind_orchestrator(elsewhere, other).unwrap();
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

#[test]
fn dev_sounds_lists_system_sounds_and_room_sound_picks_one_by_name() {
    let (mut worker, room, _agent, dir) = fixture();
    let sounds = dir.join("sounds");
    std::fs::create_dir_all(&sounds).unwrap();
    for name in ["Glass.aiff", "Windows Notify.wav", "notes.txt"] {
        std::fs::write(sounds.join(name), b"").unwrap();
    }
    worker.sound_dirs = Some(vec![sounds.clone()]);

    let listed = call(&mut worker, "sounds", "sounds", json!({}));
    assert!(listed.ok, "{listed:?}");
    assert_eq!(
        listed.result["sounds"],
        json!([
            {"name": "Default", "path": null},
            {"name": "Glass", "path": sounds.join("Glass.aiff")},
            {"name": "Windows Notify", "path": sounds.join("Windows Notify.wav")},
        ])
    );
    let state = call(&mut worker, "sound-name-state-1", "state", json!({}));
    assert_eq!(state.result["rooms"][1]["sound_name"], "Default");

    let pick = call(
        &mut worker,
        "sound-pick",
        "room.sound",
        json!({"room":"test","on":true,"sound":"glass"}),
    );
    assert!(pick.ok, "{pick:?}");
    let saved = worker.store.load().unwrap().unwrap();
    let saved = saved.room(room).unwrap();
    assert_eq!(
        saved.sound_name.as_deref(),
        Some("Glass"),
        "stored by its listed name"
    );
    assert!(saved.sound_enabled());
    let state = call(&mut worker, "sound-name-state-2", "state", json!({}));
    assert_eq!(state.result["rooms"][1]["sound_name"], "Glass");

    // An unknown name changes nothing, not even the on/off choice.
    let unknown = call(
        &mut worker,
        "sound-unknown",
        "room.sound",
        json!({"room":"test","on":false,"sound":"Sosumi"}),
    );
    assert!(!unknown.ok);
    assert!(worker.state.room(room).unwrap().sound_enabled());
    assert_eq!(
        worker.state.room(room).unwrap().sound_name.as_deref(),
        Some("Glass")
    );

    // Without --sound the choice stays; Default restores Bus's own ding.
    let off = call(
        &mut worker,
        "sound-keep",
        "room.sound",
        json!({"room":"test","on":false}),
    );
    assert!(off.ok, "{off:?}");
    assert_eq!(
        worker.state.room(room).unwrap().sound_name.as_deref(),
        Some("Glass")
    );
    let reset = call(
        &mut worker,
        "sound-default",
        "room.sound",
        json!({"room":"test","on":true,"sound":"default"}),
    );
    assert!(reset.ok, "{reset:?}");
    assert_eq!(worker.state.room(room).unwrap().sound_name, None);

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_visible_read_works_while_launching_but_keeps_terminal_identity_checks() {
    struct LaunchingInspect {
        pane: &'static str,
    }
    impl Transport for LaunchingInspect {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                // The native pane may already report a session Bus has not bound yet.
                Method::AgentGet(_) => Ok(ResponseResult::AgentInfo {
                    agent: owned_agent_info(self.pane, "bus-r1-a2", Some("unbound")),
                }),
                Method::AgentRead(params) => Ok(ResponseResult::PaneRead {
                    read: schema::PaneReadResult {
                        pane_id: params.target,
                        workspace_id: "w1".into(),
                        tab_id: "t1".into(),
                        source: schema::ReadSource::Visible,
                        format: schema::ReadFormat::Text,
                        text: "Is this a project you trust?".into(),
                        revision: 1,
                        truncated: false,
                        viewport_rows: Some(50),
                        viewport_columns: Some(171),
                        requested_lines: None,
                        returned_lines: 50,
                        available_lines: None,
                        exhausted: None,
                    },
                }),
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
                session_id: None,
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
            },
        )
        .unwrap();
    worker.transport = Box::new(LaunchingInspect { pane: "w1:p2" });
    let read = call(
        &mut worker,
        "read-launching",
        "agent.read",
        json!({"agent":"codex1","source":"visible"}),
    );
    assert!(read.ok, "{read:?}");
    assert_eq!(read.result["status"], "launching");
    assert_eq!(read.result["text"], "Is this a project you trust?");
    assert_eq!(read.result["runtime"]["session_id"], Value::Null);
    assert_eq!(read.result["runtime"]["session_verified"], false);

    // A different pane behind the same target is still refused.
    worker.transport = Box::new(LaunchingInspect { pane: "w1:other" });
    let stale = call(
        &mut worker,
        "read-launching-stale",
        "agent.read",
        json!({"agent":"codex1","source":"visible"}),
    );
    assert_eq!(
        stale.error.unwrap().message,
        "Agent terminal identity changed; inspect the owned session"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn opening_a_session_with_a_legacy_work_room_named_master_keeps_the_name_unique() {
    let dir = std::env::temp_dir().join(format!(
        "bus-control-legacy-master-{}-{}-{}",
        std::process::id(),
        super::super::super::io::now_ns(),
        NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    super::super::super::io::private_dir(&dir).unwrap();
    // Before MASTER existed, any room name was accepted.
    let mut legacy = BusState::default();
    let room = legacy.create_room("plans").unwrap();
    let mut saved = serde_json::to_value(&legacy).unwrap();
    saved["rooms"][room.0.to_string()]["name"] = json!("Master");
    let legacy: BusState = serde_json::from_value(saved).unwrap();
    JsonStore::new(dir.join("state.json"))
        .save(&legacy)
        .unwrap();

    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    worker.dev_enabled = true;
    let named_master = worker
        .state
        .rooms()
        .filter(|r| r.name.eq_ignore_ascii_case(MASTER_ROOM_NAME))
        .count();
    assert_eq!(
        named_master,
        1,
        "only the MASTER room may carry its name: {:?}",
        worker
            .state
            .rooms()
            .map(|r| (&r.name, r.kind))
            .collect::<Vec<_>>()
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deleting_a_room_deletes_its_orchestrator_from_master() {
    let (mut worker, room, codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let next = worker.state.create_room("next").unwrap();
    let orchestrator = worker
        .state
        .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    worker.state.bind_orchestrator(orchestrator, room).unwrap();
    let other = worker
        .state
        .create_agent(master, "next-orch", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker.state.bind_orchestrator(other, next).unwrap();

    let deleted = call(
        &mut worker,
        "delete-orchestrated",
        "room.delete",
        json!({"room":"test","confirm":true}),
    );
    assert!(deleted.ok, "{deleted:?}");
    assert!(worker.state.room(room).is_none());
    assert!(worker.state.agent(codex).is_none());
    assert!(worker.state.agent(orchestrator).is_none());
    // Another room's orchestrator is untouched.
    assert_eq!(
        worker.state.orchestrator_of(next).map(|a| a.id),
        Some(other)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn saved_work_room_named_master_keeps_its_notes_when_addressed_by_name() {
    let dir = std::env::temp_dir().join(format!(
        "bus-control-named-master-{}-{}-{}",
        std::process::id(),
        super::super::super::io::now_ns(),
        NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    super::super::super::io::private_dir(&dir).unwrap();
    let mut legacy = BusState::default();
    let room = legacy.create_room("plans").unwrap();
    legacy.set_room_notes(room, "keep-notes").unwrap();
    legacy.set_draft_text(room, "keep-draft").unwrap();
    let mut saved = serde_json::to_value(&legacy).unwrap();
    saved["rooms"][room.0.to_string()]["name"] = json!("Master");
    saved["rooms"][room.0.to_string()]["unread_count"] = json!(4);
    let legacy: BusState = serde_json::from_value(saved).unwrap();
    JsonStore::new(dir.join("state.json"))
        .save(&legacy)
        .unwrap();

    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    worker.dev_enabled = true;
    assert_eq!(worker.state.room(room).unwrap().notes, "keep-notes");
    assert_eq!(worker.state.room(room).unwrap().draft.text, "keep-draft");
    assert_eq!(worker.state.room(room).unwrap().unread_count, 4);
    assert_eq!(worker.state.room(room).unwrap().kind, RoomKind::Work);
    assert_eq!(
        worker
            .state
            .rooms()
            .filter(|r| r.name.eq_ignore_ascii_case(MASTER_ROOM_NAME))
            .count(),
        1,
        "a saved work room named Master must not share that name with MASTER: {:?}",
        worker
            .state
            .rooms()
            .map(|r| (&r.name, r.kind))
            .collect::<Vec<_>>()
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_as_an_ambiguous_room_agent_name_never_falls_back_to_the_orchestrator() {
    let (mut worker, room, _codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    for _ in 0..2 {
        worker
            .state
            .create_agent(room, "dev", Provider::Codex, dir.clone(), None)
            .unwrap();
    }
    let orchestrator = worker
        .state
        .create_agent(master, "dev", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    worker.state.bind_orchestrator(orchestrator, room).unwrap();
    let sent = call(
        &mut worker,
        "as-ambiguous",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"x","as":"dev"}),
    );
    assert!(
        error_message(&sent).contains("Ambiguous"),
        "an ambiguous author must not resolve to the orchestrator: {sent:?}"
    );
    assert_eq!(worker.state.requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_as_counts_the_agent_message_as_unread_in_a_room_out_of_view() {
    let (mut worker, room, codex, dir) = fixture();
    let claude = worker
        .state
        .create_agent(room, "claude1", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    assert_ne!(worker.state.visible_room(), Some(room));
    let human = call(
        &mut worker,
        "unread-human",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"from the human"}),
    );
    assert!(human.ok, "{human:?}");
    assert_eq!(worker.state.room(room).unwrap().unread_count, 0);

    let agent = call(
        &mut worker,
        "unread-agent",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"from claude","as":"claude1"}),
    );
    assert!(agent.ok, "{agent:?}");
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);

    // A visible room stays read, as it does for replies.
    worker.state.select_room(room).unwrap();
    worker.state.mark_room_seen(room).unwrap();
    let seen = call(
        &mut worker,
        "unread-visible",
        "message.send",
        json!({"room":"test","to":[codex.0.to_string()],"text":"again","as":claude.0.to_string()}),
    );
    assert!(seen.ok, "{seen:?}");
    assert_eq!(worker.state.room(room).unwrap().unread_count, 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Rejects the agent's tab, so an add stops after Bus prepared its folder.
struct NoTabs;
impl Transport for NoTabs {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        assert!(matches!(method, Method::TabCreate(_)), "{method:?}");
        Err(TransportError {
            code: None,
            message: "no tabs in tests".into(),
            definitely_rejected: true,
        })
    }
}

fn launch_spool(worker: &Worker, agent: AgentId) -> PathBuf {
    let launch = worker
        .state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .launch_id
        .clone();
    worker.data_dir.join("callbacks").join(launch.unwrap())
}

fn messages_to(worker: &Worker, agent: AgentId) -> Vec<String> {
    worker
        .state
        .requests()
        .filter(|r| r.agent_id == agent)
        .map(|r| r.prompt.text.clone())
        .collect()
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
        crate::bus::orchestrator::resume_prompt_args(Provider::ClaudeCode, &spool).unwrap()[..2],
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
        crate::bus::io::now_ns()
    ));
    crate::bus::io::private_dir(&dir).unwrap();
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
        crate::bus::io::now_ns()
    ));
    crate::bus::io::private_dir(&dir).unwrap();
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
fn state_reports_how_the_running_bus_was_built() {
    let (mut worker, _room, _agent, _dir) = fixture();
    let state = call(&mut worker, "state-build", "state", json!({}));
    let expected = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    assert_eq!(state.result["build"]["profile"], expected);
    assert_eq!(
        state.result["build"]["binary"],
        json!(std::env::current_exe().unwrap())
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
            super::super::super::io::now_ns(),
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
fn a_long_session_of_mutations_never_hits_a_permanent_receipt_limit() {
    let (mut worker, _room, _agent, dir) = fixture();
    // Retries within the retention window replay; older receipts make room.
    worker.dev_receipt_retention = Duration::ZERO;
    for index in 0..1100 {
        let seen = call(
            &mut worker,
            &format!("seen-{index}"),
            "room.seen",
            json!({"room":"test"}),
        );
        assert!(seen.ok, "{index}: {seen:?}");
    }
    assert!(worker.dev_receipts.len() <= 1);

    worker.dev_receipt_retention = Duration::from_secs(600);
    let first = call(&mut worker, "kept", "room.seen", json!({"room":"test"}));
    assert!(first.ok, "{first:?}");
    let conflict = call(&mut worker, "kept", "room.seen", json!({"room":"other"}));
    assert!(
        error_message(&conflict).contains("already used"),
        "{conflict:?}"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_to_human_posts_an_orchestrator_report_in_master_history() {
    let (mut worker, room, _codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
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
    worker.state.bind_orchestrator(orchestrator, room).unwrap();
    assert_ne!(worker.state.visible_room(), Some(master));

    let sent = call(
        &mut worker,
        "report",
        "message.send",
        json!({"room":"master","to":["human"],"text":"gate passed; please restart","as":"claude-orch"}),
    );
    assert!(sent.ok, "{sent:?}");
    let result = &sent.result;
    assert_eq!(result["stage"], "posted");
    assert_eq!(result["request_ids"], json!([]));
    // No agent receives it, so it creates no request.
    assert_eq!(worker.state.requests().count(), 0);
    let master_room = worker.state.room(master).unwrap();
    let report = master_room.notices.last().unwrap();
    assert_eq!(report.author, Author::Agent(orchestrator));
    assert_eq!(report.text, "gate passed; please restart");
    assert!(report.recipient_ids.is_empty());
    assert_eq!(result["message_id"], json!(report.id));
    // The latest prompt is what rings and counts as unread, like a reply.
    assert_eq!(master_room.latest_prompt.as_ref(), Some(report));
    assert_eq!(master_room.unread_count, 1);

    for (id, params, expected) in [
        (
            "no-author",
            json!({"room":"master","to":["human"],"text":"x"}),
            "requires --as",
        ),
        (
            "work-room",
            json!({"room":"test","to":["human"],"text":"x","as":"claude-orch"}),
            "only in the MASTER room",
        ),
        (
            "mixed",
            json!({"room":"master","to":["human","claude-orch"],"text":"x","as":"claude-orch"}),
            "cannot be combined",
        ),
        (
            "queued",
            json!({"room":"master","to":["human"],"text":"x","as":"claude-orch","queue":true}),
            "--queue",
        ),
        (
            "empty",
            json!({"room":"master","to":["human"],"text":"  ","as":"claude-orch"}),
            "",
        ),
    ] {
        let rejected = call(&mut worker, id, "message.send", params);
        assert!(!rejected.ok, "{id}: {rejected:?}");
        assert!(
            error_message(&rejected).contains(expected),
            "{id}: {rejected:?}"
        );
    }
    assert_eq!(worker.state.room(master).unwrap().notices.len(), 1);
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

#[test]
fn notices_older_versions_saved_as_bus_are_dropped_and_reports_to_the_human_kept() {
    let (mut worker, _room, _codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let orchestrator = worker
        .state
        .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    worker
        .state
        .post_to_human(master, orchestrator, "first report".into(), Vec::new(), 1)
        .unwrap();
    let mut saved = serde_json::to_value(&worker.state).unwrap();
    let notices = &mut saved["rooms"][master.0.to_string()]["notices"];
    let mut legacy = notices[0].clone();
    legacy["author"] = json!("bus");
    legacy["text"] = json!("orch never started message 9");
    notices.as_array_mut().unwrap().push(legacy);
    let loaded: BusState = serde_json::from_value(saved).unwrap();
    let notices = &loaded.room(master).unwrap().notices;
    assert_eq!(notices.len(), 1, "Bus is not an agent and authors nothing");
    assert_eq!(notices[0].text, "first report");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn message_status_reports_whether_each_recipient_turn_ended() {
    let (mut worker, _room, _codex, dir) = fixture();
    let sent = call(
        &mut worker,
        "async-send",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"go"}),
    );
    assert!(sent.ok, "{sent:?}");
    let message = sent.result["message_id"].to_string();
    let status = call(
        &mut worker,
        "async-status",
        "message.status",
        json!({ "message": message }),
    );
    // Still queued: `send --async` keeps following until this turns true.
    assert_eq!(
        status.result["requests"][0]["turn_ended"], false,
        "{status:?}"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn message_status_shows_a_stalled_stage_with_its_reason() {
    let (mut worker, _room, codex, dir) = fixture();
    let sent = call(
        &mut worker,
        "stall-send",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"go"}),
    );
    let message = PromptId(sent.result["message_id"].as_u64().unwrap());
    let now = crate::bus::io::now_ms();
    worker
        .state
        .observe_status(codex, RuntimeStatus::Idle, now)
        .unwrap();
    let request = |status: &serde_json::Value| status["requests"][0].clone();

    let fresh = request(&worker.dev_message_at(message, now).unwrap());
    assert_eq!(fresh["stage"], "queued");
    assert!(fresh["stalled_from"].is_null());

    let later = now + crate::bus::model::QUEUED_STALL_MS;
    let stalled = request(&worker.dev_message_at(message, later).unwrap());
    assert_eq!(stalled["stage"], "stalled", "{stalled}");
    assert_eq!(stalled["stalled_from"], "queued");
    let expected = crate::bus::diagnostics::wait_reason(worker.state.agent(codex).unwrap());
    assert_eq!(
        stalled["reason"],
        json!(expected.unwrap_or("not_submitted"))
    );

    // A blocked agent is reported, not stalled.
    worker
        .state
        .observe_status(codex, RuntimeStatus::Blocked, later)
        .unwrap();
    let much_later = later + crate::bus::model::BLOCKED_STALL_MS;
    let blocked = request(&worker.dev_message_at(message, much_later).unwrap());
    assert_eq!(blocked["stage"], "queued", "{blocked}");
    assert_eq!(blocked["reason"], "blocked_unanswered");

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_queued_question_redraw_returns_a_fingerprint_then_allows_choose_and_answer() {
    use std::sync::{Arc, Mutex};
    struct Native {
        stage: u32,
        opens: Vec<bool>,
    }
    struct QueuedTransport(Arc<Mutex<Native>>);
    fn observation(stage: u32) -> schema::AgentDialogObservation {
        let screen = match stage {
            1 => include_str!("../../tests/fixtures/codex-question/expanded.txt"),
            2 => include_str!("../../tests/fixtures/codex-question/other-selected.txt"),
            _ => "",
        };
        let dialog = crate::detect::dialog::parse(screen).map(|dialog| schema::AgentDialog {
            kind: match dialog.kind {
                crate::detect::dialog::DialogKind::Choice => schema::AgentDialogKind::Choice,
                crate::detect::dialog::DialogKind::Question => schema::AgentDialogKind::Question,
            },
            id: dialog.id(),
            digest: dialog.digest(),
            text: dialog.text,
            hint: dialog.hint,
            options: dialog
                .options
                .into_iter()
                .map(|option| schema::AgentDialogOption {
                    number: option.number,
                    label: option.label,
                    selected: option.selected,
                })
                .collect(),
        });
        schema::AgentDialogObservation {
            pending_question: stage == 0,
            terminal_id: "terminal".into(),
            pane_id: "pane".into(),
            session_id: None,
            content_revision: stage as u64,
            dialog,
        }
    }
    impl Transport for QueuedTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let mut native = self.0.lock().unwrap();
            match method {
                Method::AgentDialogObserve(params) => {
                    native.opens.push(params.open_pending_question);
                    if native.stage == 0 && native.opens.len() == 3 {
                        native.stage = 1;
                    }
                    Ok(ResponseResult::AgentDialog {
                        observation: observation(native.stage),
                    })
                }
                Method::AgentDialogChoose(params) => {
                    assert_eq!(params.option, 3);
                    assert_eq!(
                        params.expected_dialog_digest,
                        observation(1).dialog.unwrap().digest
                    );
                    native.stage = 2;
                    Ok(ResponseResult::AgentDialogChosen {
                        choice: schema::AgentDialogChooseResult {
                            written: true,
                            reason: None,
                            keys: vec!["down".into(), "down".into(), "enter".into()],
                            observation: observation(2),
                        },
                    })
                }
                Method::AgentDialogAnswer(params) => {
                    assert_eq!(params.text.as_deref(), Some("capture answer"));
                    assert_eq!(
                        params.expected_dialog_digest,
                        observation(2).dialog.unwrap().digest
                    );
                    native.stage = 3;
                    Ok(ResponseResult::AgentDialogChosen {
                        choice: schema::AgentDialogChooseResult {
                            written: true,
                            reason: None,
                            keys: vec!["enter".into()],
                            observation: observation(3),
                        },
                    })
                }
                other => panic!("unexpected request: {other:?}"),
            }
        }
    }
    let (mut worker, _, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: None,
            },
        )
        .unwrap();
    let native = Arc::new(Mutex::new(Native {
        stage: 0,
        opens: Vec::new(),
    }));
    worker.transport = Box::new(QueuedTransport(native.clone()));
    let observed = worker.observe_dialog(agent).unwrap();
    assert_eq!(
        native.lock().unwrap().opens,
        [true, false, false],
        "open only once, then poll without navigation"
    );
    assert_eq!(observed["dialog"]["options"][2]["label"], "Other");
    let fingerprint = observed["fingerprint"].as_str().unwrap();
    assert_eq!(
        worker.choose_dialog_option(agent, 3, fingerprint).unwrap()["outcome"],
        "replaced"
    );
    assert!(worker.choose_dialog_option(agent, 3, fingerprint).is_err());
    let other = worker.observe_dialog(agent).unwrap();
    assert_eq!(other["dialog"]["kind"], "question");
    let fingerprint = other["fingerprint"].as_str().unwrap();
    assert_eq!(
        worker
            .answer_dialog(agent, Some("capture answer"), false, fingerprint)
            .unwrap()["outcome"],
        "closed"
    );
    assert!(worker
        .answer_dialog(agent, Some("replay"), false, fingerprint)
        .is_err());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
