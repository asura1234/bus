use super::*;

#[test]
fn request_uses_dot_method_names() {
    let request = Request {
        id: "req_1".into(),
        method: Method::WorkspaceCreate(WorkspaceCreateParams {
            source_workspace_id: None,
            cwd: Some("/tmp".into()),
            focus: true,
            label: Some("api".into()),
            env: Default::default(),
        }),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "workspace.create");
}

#[test]
fn agent_start_and_prompt_requests_round_trip() {
    let start = Request {
        id: "start".into(),
        method: Method::AgentStart(AgentStartParams {
            name: "reviewer".into(),
            kind: "pi".into(),
            pane_id: "w1:p2".into(),
            args: vec!["--no-session".into()],
            timeout_ms: Some(30_000),
        }),
    };
    let start_json = serde_json::to_value(&start).unwrap();
    assert_eq!(start_json["method"], "agent.start");
    assert_eq!(start_json["params"]["pane_id"], "w1:p2");
    assert_eq!(
        serde_json::from_value::<Request>(start_json).unwrap(),
        start
    );

    let prompt = Request {
        id: "prompt".into(),
        method: Method::AgentPrompt(AgentPromptParams {
            target: "reviewer".into(),
            text: "review this".into(),
            wait: None,
        }),
    };
    let prompt_json = serde_json::to_value(&prompt).unwrap();
    assert_eq!(prompt_json["method"], "agent.prompt");
    assert_eq!(
        serde_json::from_value::<Request>(prompt_json).unwrap(),
        prompt
    );

    let prompt_and_wait = Request {
        id: "prompt-and-wait".into(),
        method: Method::AgentPrompt(AgentPromptParams {
            target: "reviewer".into(),
            text: "review this".into(),
            wait: Some(AgentPromptWaitOptions {
                until: vec![AgentStatus::Idle, AgentStatus::Done],
                timeout_ms: Some(120_000),
                submission_deadline: None,
            }),
        }),
    };
    let prompt_and_wait_json = serde_json::to_value(&prompt_and_wait).unwrap();
    assert_eq!(
        prompt_and_wait_json["params"]["wait"]["until"],
        serde_json::json!(["idle", "done"])
    );
    assert_eq!(
        prompt_and_wait_json["params"]["wait"]["timeout_ms"],
        120_000
    );
    assert_eq!(
        serde_json::from_value::<Request>(prompt_and_wait_json).unwrap(),
        prompt_and_wait
    );
}

#[test]
fn request_round_trips_for_server_stop() {
    let request = Request {
        id: "req_stop".into(),
        method: Method::ServerStop(EmptyParams::default()),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "server.stop");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, request);
}

#[test]
fn request_round_trips_for_server_reload_config() {
    let request = Request {
        id: "req_reload".into(),
        method: Method::ServerReloadConfig(EmptyParams::default()),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "server.reload_config");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, request);
}

#[test]
fn notification_show_request_parses() {
    let json = r#"{"id":"req_1","method":"notification.show","params":{"title":"build failed","body":"api workspace","position":"top-left","sound":"request"}}"#;
    let request: Request = serde_json::from_str(json).unwrap();
    let Method::NotificationShow(params) = request.method else {
        panic!("wrong method parsed");
    };
    assert_eq!(params.title, "build failed");
    assert_eq!(params.body.as_deref(), Some("api workspace"));
    assert_eq!(
        params.position,
        Some(crate::utils::config::ToastHerdrPosition::TopLeft)
    );
    assert_eq!(params.sound, NotificationShowSound::Request);
}

#[test]
fn notification_show_sound_defaults_to_none() {
    let json = r#"{"id":"req_1","method":"notification.show","params":{"title":"build failed"}}"#;
    let request: Request = serde_json::from_str(json).unwrap();
    let Method::NotificationShow(params) = request.method else {
        panic!("wrong method parsed");
    };

    assert_eq!(params.sound, NotificationShowSound::None);
}

#[test]
fn client_window_title_requests_round_trip() {
    let set = Request {
        id: "req_title_set".into(),
        method: Method::ClientWindowTitleSet(ClientWindowTitleSetParams {
            title: "herdr api".into(),
        }),
    };
    let json = serde_json::to_value(&set).unwrap();
    assert_eq!(json["method"], "client.window_title.set");
    assert_eq!(json["params"]["title"], "herdr api");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, set);

    let clear = Request {
        id: "req_title_clear".into(),
        method: Method::ClientWindowTitleClear(EmptyParams::default()),
    };
    let json = serde_json::to_value(&clear).unwrap();
    assert_eq!(json["method"], "client.window_title.clear");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, clear);
}

#[test]
fn unknown_method_is_rejected() {
    let json = r#"{"id":"req_1","method":"nope","params":{}}"#;
    let err = serde_json::from_str::<Request>(json)
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown variant"));
}

#[test]
fn missing_required_params_are_rejected() {
    let json = r#"{"id":"req_1","method":"pane.send_text","params":{"pane_id":"p_1"}}"#;
    let err = serde_json::from_str::<Request>(json)
        .unwrap_err()
        .to_string();
    assert!(err.contains("text"));
}

#[test]
fn pane_send_input_defaults_to_empty_text_and_keys() {
    let json = r#"
    {
        "id": "req_1",
        "method": "pane.send_input",
        "params": {
            "pane_id": "p_1"
        }
    }
    "#;

    let request: Request = serde_json::from_str(json).unwrap();
    let Method::PaneSendInput(params) = request.method else {
        panic!("wrong method parsed");
    };
    assert_eq!(params.pane_id, "p_1");
    assert!(params.text.is_empty());
    assert!(params.keys.is_empty());
}

#[test]
fn pane_wait_for_output_defaults_strip_ansi_to_true() {
    let json = r#"
    {
        "id": "req_1",
        "method": "pane.wait_for_output",
        "params": {
            "pane_id": "p_1",
            "source": "recent",
            "match": { "type": "substring", "value": "ready" }
        }
    }
    "#;

    let request: Request = serde_json::from_str(json).unwrap();
    let Method::PaneWaitForOutput(params) = request.method else {
        panic!("wrong method parsed");
    };
    assert!(params.strip_ansi);
}

#[test]
fn room_orchestrator_core_pane_read_schema_preserves_native_optional_lines_and_fields() {
    let json = r#"
    {
        "id": "req_1",
        "method": "pane.read",
        "params": {
            "pane_id": "p_1",
            "source": "visible"
        }
    }
    "#;

    let request: Request = serde_json::from_str(json).unwrap();
    let serialized = serde_json::to_value(&request).unwrap();
    assert!(serialized["params"].get("intent").is_none());
    let Method::PaneRead(params) = request.method else {
        panic!("wrong method parsed");
    };
    assert_eq!(params.format, ReadFormat::Text);
    assert_eq!(params.intent, ReadIntent::Interactive);
}

#[test]
fn pane_current_request_round_trips() {
    let request = Request {
        id: "req_current".into(),
        method: Method::PaneCurrent(PaneCurrentParams {
            caller_pane_id: Some("w1-1".into()),
        }),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "pane.current");
    assert_eq!(json["params"]["caller_pane_id"], "w1-1");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, request);
}

#[test]
fn subscribe_request_parses_parameterized_subscriptions() {
    let json = r#"
    {
        "id": "sub_1",
        "method": "events.subscribe",
        "params": {
            "subscriptions": [
                {
                    "type": "pane.output_matched",
                    "pane_id": "p_1_1",
                    "source": "recent",
                    "lines": 200,
                    "match": { "type": "substring", "value": "auth: received" }
                },
                {
                    "type": "pane.agent_status_changed",
                    "pane_id": "p_1_1",
                    "agent_status": "done"
                },
                {
                    "type": "pane.scroll_changed",
                    "pane_id": "p_1_1"
                }
            ]
        }
    }
    "#;

    let request: Request = serde_json::from_str(json).unwrap();
    let Method::EventsSubscribe(params) = request.method else {
        panic!("wrong method parsed");
    };
    assert_eq!(params.subscriptions.len(), 3);
    assert!(matches!(
        &params.subscriptions[0],
        Subscription::PaneOutputMatched {
            pane_id,
            source: ReadSource::Recent,
            lines: Some(200),
            r#match: OutputMatch::Substring { value },
            strip_ansi: true,
        } if pane_id == "p_1_1" && value == "auth: received"
    ));
    assert!(matches!(
        &params.subscriptions[1],
        Subscription::PaneAgentStatusChanged {
            pane_id,
            agent_status: Some(AgentStatus::Done),
        } if pane_id == "p_1_1"
    ));
    assert!(matches!(
        &params.subscriptions[2],
        Subscription::PaneScrollChanged { pane_id } if pane_id == "p_1_1"
    ));
}

#[test]
fn agent_dialog_choose_schema_has_no_launch_or_arbitrary_keys() {
    let request = Request {
        id: "choose-1".into(),
        method: Method::AgentDialogChoose(AgentDialogChooseParams {
            target: "w1:p2".into(),
            expected_terminal_id: "terminal-1".into(),
            expected_pane_id: "w1:p2".into(),
            expected_session_id: None,
            expected_dialog_digest: "abc".into(),
            option: 2,
        }),
    };
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["method"], "agent.dialog.choose");
    let params = &value["params"];
    assert!(params.get("expected_session_id").is_none());
    assert!(params.get("keys").is_none());
    assert_eq!(serde_json::from_value::<Request>(value).unwrap(), request);
    assert!(serde_json::from_value::<Request>(serde_json::json!({
        "id":"x","method":"agent.dialog.choose","params":{
            "target":"w1:p2","expected_terminal_id":"terminal-1","expected_pane_id":"w1:p2",
            "expected_dialog_digest":"abc","option":1,"keys":["enter"]
        }
    }))
    .is_err());
}

#[test]
fn agent_dialog_answer_schema_round_trips_and_old_dialogs_default_to_choice() {
    let request = Request {
        id: "answer".into(),
        method: Method::AgentDialogAnswer(AgentDialogAnswerParams {
            target: "pane".into(),
            expected_terminal_id: "terminal".into(),
            expected_pane_id: "pane".into(),
            expected_session_id: None,
            expected_dialog_digest: "digest".into(),
            text: Some("token".into()),
            skip: false,
        }),
    };
    let mut value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["method"], "agent.dialog.answer");
    assert_eq!(
        serde_json::from_value::<Request>(value.clone()).unwrap(),
        request
    );
    value["params"]["keys"] = serde_json::json!(["enter"]);
    assert!(serde_json::from_value::<Request>(value).is_err());
    let old = serde_json::json!({"text":"Allow?","options":[],"id":"id","digest":"digest"});
    assert_eq!(
        serde_json::from_value::<AgentDialog>(old).unwrap().kind,
        AgentDialogKind::Choice
    );
}

#[test]
fn agent_status_request_values_remain_strict() {
    assert!(serde_json::from_str::<AgentStatus>(r#""working""#).is_ok());
    assert!(serde_json::from_str::<AgentStatus>(r#""future_status""#).is_err());
}

#[test]
fn authority_mutation_requests_round_trip() {
    let workspace_move = Request {
        id: "move_ws".into(),
        method: Method::WorkspaceMove(WorkspaceMoveParams {
            workspace_id: "w1".into(),
            insert_index: 2,
        }),
    };
    let json = serde_json::to_value(&workspace_move).unwrap();
    assert_eq!(json["method"], "workspace.move");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, workspace_move);

    let pane_focus = Request {
        id: "focus_pane".into(),
        method: Method::PaneFocus(PaneTarget {
            pane_id: "w1:1".into(),
        }),
    };
    let json = serde_json::to_value(&pane_focus).unwrap();
    assert_eq!(json["method"], "pane.focus");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, pane_focus);

    let split_ratio = Request {
        id: "set_ratio".into(),
        method: Method::LayoutSetSplitRatio(LayoutSetSplitRatioParams {
            tab_id: Some("w1:1".into()),
            pane_id: None,
            path: vec![false, true],
            ratio: 0.6,
        }),
    };
    let json = serde_json::to_value(&split_ratio).unwrap();
    assert_eq!(json["method"], "layout.set_split_ratio");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, split_ratio);

    let subscription = Request {
        id: "sub_moves".into(),
        method: Method::EventsSubscribe(EventsSubscribeParams {
            subscriptions: vec![
                Subscription::WorkspaceMoved {},
                Subscription::LayoutUpdated {},
            ],
        }),
    };
    let json = serde_json::to_string(&subscription).unwrap();
    assert!(json.contains("\"type\":\"workspace.moved\""));
    assert!(json.contains("\"type\":\"layout.updated\""));
    let restored: Request = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, subscription);
}

#[test]
fn event_wait_parses_typed_match() {
    let json = r#"
    {
        "id": "req_9",
        "method": "events.wait",
        "params": {
            "match_event": {
                "event": "pane_agent_status_changed",
                "pane_id": "p_1",
                "agent_status": "done"
            },
            "timeout_ms": 30000
        }
    }
    "#;

    let request: Request = serde_json::from_str(json).unwrap();
    let Method::EventsWait(params) = request.method else {
        panic!("wrong method parsed");
    };
    assert_eq!(
        params.match_event,
        EventMatch::PaneAgentStatusChanged {
            pane_id: "p_1".into(),
            agent_status: AgentStatus::Done,
        }
    );
}

#[test]
fn pane_link_activate_round_trips() {
    let request = Request {
        id: "req_pane_link".into(),
        method: Method::PaneLinkActivate(PaneLinkActivateParams {
            pane_id: "w1:p1".into(),
            viewport_row: 3,
            col: 7,
            content_revision: Some(42),
            offset_from_bottom: Some(5),
        }),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "pane.link.activate");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, request);

    let response = ResponseResult::PaneLinkActivated {
        url: Some("https://example.test".into()),
        handled: false,
    };
    let json = serde_json::to_string(&response).unwrap();
    let restored: ResponseResult = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}
