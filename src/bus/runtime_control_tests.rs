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
        super::super::super::io::now_ms()
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
fn dev_send_preserves_draft_and_targets_exactly_one_agent() {
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
fn dev_recovery_abandons_an_idle_wedged_request_without_replacing_the_agent() {
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
fn dev_read_uses_native_pane_selector_not_internal_terminal_id() {
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
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
                ..Default::default()
            },
        )
        .unwrap();
    worker.transport = Box::new(InspectTarget);
    let result = call(&mut worker, "read", "agent.read", json!({"agent":"codex1"}));
    assert_eq!(result.error.unwrap().message, "probe complete");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_read_visible_returns_bounded_snapshot_and_correlation_metadata() {
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
    assert_eq!(result.result["capture"]["more"], false);
    assert_eq!(result.result["capture"]["revision"], 9);
    assert!(result.result["capture"].get("lines").is_none());
    assert_eq!(result.result["capture"]["viewport"]["rows"], 24);
    assert_eq!(
        result.result["capture"]["viewport"]["offset_from_bottom"],
        0
    );
    assert_eq!(
        result.result["capture"]["viewport"]["max_offset_from_bottom"],
        12
    );
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
fn dev_read_visible_fails_closed_for_missing_or_stale_runtime_identity() {
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
fn dev_read_without_source_keeps_recent_output_contract() {
    struct RecentInspect;
    impl Transport for RecentInspect {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentGet(params) => {
                    assert_eq!(params.target, "w1:p2");
                    Ok(ResponseResult::AgentInfo {
                        agent: owned_agent_info("w1:p2", "bus-r1-a2", None),
                    })
                }
                Method::AgentRead(params) => {
                    assert_eq!(params.target, "w1:p2");
                    assert_eq!(params.source, schema::ReadSource::Recent);
                    assert_eq!(params.lines, Some(400));
                    Ok(ResponseResult::PaneRead {
                        read: schema::PaneReadResult {
                            pane_id: "w1:p2".into(),
                            workspace_id: "w1".into(),
                            tab_id: "t1".into(),
                            source: schema::ReadSource::Recent,
                            format: schema::ReadFormat::Text,
                            text: "recent history".into(),
                            revision: 4,
                            truncated: false,
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
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
                ..Default::default()
            },
        )
        .unwrap();
    worker.transport = Box::new(RecentInspect);
    let result = call(&mut worker, "read", "agent.read", json!({"agent":"codex1"}));
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["agent_id"], agent.0);
    assert_eq!(result.result["runtime"]["pane_id"], "w1:p2");
    assert_eq!(result.result["output"]["type"], "pane_read");
    assert_eq!(result.result["output"]["read"]["text"], "recent history");
    assert!(result.result.get("text").is_none() || result.result["text"].is_null());
    assert!(result.result.get("capture").is_none() || result.result["capture"].is_null());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_read_recent_uses_caller_selected_native_lines_and_reports_more() {
    struct RecentRange;
    impl Transport for RecentRange {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentGet(params) => {
                    assert_eq!(params.target, "w1:p2");
                    Ok(ResponseResult::AgentInfo {
                        agent: owned_agent_info("w1:p2", "bus-r1-a2", None),
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
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
                ..Default::default()
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
    assert_eq!(result.result["capture"]["more"], true);
    assert_eq!(result.result["capture"]["resumable"], true);
    assert_eq!(result.result["capture"]["revision"], 11);
    assert_eq!(result.result["capture"]["lines"], 80);
    assert_eq!(result.result["capture"]["requested_lines"], 80);
    assert_eq!(result.result["capture"]["native_max_lines"], 1000);
    assert!(result.result["capture"].get("limit").is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_read_recent_surfaces_native_line_cap_without_silent_truncation() {
    struct CappedRecent;
    impl Transport for CappedRecent {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            match method {
                Method::AgentGet(_) => Ok(ResponseResult::AgentInfo {
                    agent: owned_agent_info("w1:p2", "bus-r1-a2", None),
                }),
                Method::AgentRead(params) => {
                    assert_eq!(params.lines, Some(1000));
                    Ok(ResponseResult::PaneRead {
                        read: schema::PaneReadResult {
                            pane_id: "w1:p2".into(),
                            workspace_id: "w1".into(),
                            tab_id: "t1".into(),
                            source: schema::ReadSource::Recent,
                            format: schema::ReadFormat::Text,
                            text: "capped history".into(),
                            revision: 2,
                            truncated: true,
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
                pane_id: Some("w1:p2".into()),
                terminal_id: Some("term_internal".into()),
                ..Default::default()
            },
        )
        .unwrap();
    worker.transport = Box::new(CappedRecent);
    let result = call(
        &mut worker,
        "read-capped",
        "agent.read",
        json!({"agent":"codex1","lines":2000}),
    );
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["capture"]["requested_lines"], 2000);
    assert_eq!(result.result["capture"]["lines"], 1000);
    assert_eq!(result.result["capture"]["native_max_lines"], 1000);
    assert_eq!(result.result["capture"]["more"], true);
    assert_eq!(result.result["capture"]["resumable"], false);
    assert_eq!(
        result.result["capture"]["limit"]["source"],
        "native AgentRead/pane.read caps lines at 1000"
    );
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
