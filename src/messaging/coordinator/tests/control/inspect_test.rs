use super::*;

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
