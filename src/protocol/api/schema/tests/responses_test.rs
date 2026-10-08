use super::*;
use std::collections::HashMap;

#[test]
fn event_envelope_round_trips() {
    let events = [
        EventEnvelope {
            event: EventKind::PaneOutputChanged,
            data: EventData::PaneOutputChanged {
                pane_id: "p_1".into(),
                workspace_id: "w_1".into(),
                revision: 42,
            },
        },
        EventEnvelope {
            event: EventKind::WorkspaceMoved,
            data: EventData::WorkspaceMoved {
                workspace_id: "w_1".into(),
                insert_index: 2,
                workspaces: vec![],
            },
        },
        EventEnvelope {
            event: EventKind::LayoutUpdated,
            data: EventData::LayoutUpdated {
                layout: PaneLayoutSnapshot {
                    workspace_id: "w_1".into(),
                    tab_id: "w_1:1".into(),
                    zoomed: false,
                    area: PaneLayoutRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 24,
                    },
                    focused_pane_id: "w_1-1".into(),
                    panes: vec![PaneLayoutPane {
                        pane_id: "w_1-1".into(),
                        focused: true,
                        rect: PaneLayoutRect {
                            x: 0,
                            y: 0,
                            width: 100,
                            height: 24,
                        },
                    }],
                    splits: vec![],
                },
            },
        },
    ];

    for event in events {
        let json = serde_json::to_string(&event).unwrap();
        let restored: EventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, event);
    }
}

#[test]
fn subscription_event_envelope_round_trips() {
    let event = SubscriptionEventEnvelope {
        event: SubscriptionEventKind::PaneOutputMatched,
        data: SubscriptionEventData::PaneOutputMatched(PaneOutputMatchedEvent {
            pane_id: "p_1_1".into(),
            matched_line: "auth: received".into(),
            read: PaneReadResult {
                pane_id: "p_1_1".into(),
                workspace_id: "w_1".into(),
                tab_id: "t_1_1".into(),
                source: ReadSource::Recent,
                format: ReadFormat::Text,
                text: "auth: received\n".into(),
                revision: 0,
                truncated: false,
                viewport_rows: None,
                viewport_columns: None,
                requested_lines: Some(20),
                returned_lines: 1,
                available_lines: Some(1),
                exhausted: Some(true),
            },
        }),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"event\":\"pane.output_matched\""));
    let restored: SubscriptionEventEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, event);
}

#[test]
fn agent_dialog_observation_round_trips_with_and_without_a_dialog() {
    for dialog in [
        None,
        Some(AgentDialog {
            kind: AgentDialogKind::Choice,
            text: "Do you want to proceed?".into(),
            options: vec![AgentDialogOption {
                number: 1,
                label: "Yes".into(),
                selected: true,
            }],
            hint: None,
            id: "id".into(),
            digest: "digest".into(),
        }),
    ] {
        let result = ResponseResult::AgentDialog {
            observation: AgentDialogObservation {
                terminal_id: "terminal-1".into(),
                pane_id: "w1:p2".into(),
                session_id: Some("session-1".into()),
                content_revision: 10,
                dialog,
            },
        };
        assert_eq!(
            serde_json::from_str::<ResponseResult>(&serde_json::to_string(&result).unwrap())
                .unwrap(),
            result
        );
    }
}

#[test]
fn scroll_changed_subscription_event_round_trips() {
    let event = SubscriptionEventEnvelope {
        event: SubscriptionEventKind::ScrollChanged,
        data: SubscriptionEventData::ScrollChanged(PaneScrollChangedEvent {
            pane_id: "p_1_1".into(),
            workspace_id: "w_1".into(),
            scroll: PaneScrollInfo {
                offset_from_bottom: 12,
                max_offset_from_bottom: 240,
                viewport_rows: 30,
            },
        }),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"event\":\"pane.scroll_changed\""));
    let restored: SubscriptionEventEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, event);
}

#[test]
fn success_response_round_trips() {
    let response = SuccessResponse {
        id: "req_1".into(),
        result: ResponseResult::Pong {
            version: "0.1.2".into(),
            protocol: 6,
            capabilities: Some(ServerCapabilities {
                detached_server_daemon: true,
                endpoint_protocol_generation: Some(1),
                surface_interest: true,
                health_check: true,
            }),
        },
    };

    let json = serde_json::to_string(&response).unwrap();
    let restored: SuccessResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}

#[test]
fn session_snapshot_request_and_response_round_trip() {
    let request = Request {
        id: "req_snapshot".into(),
        method: Method::SessionSnapshot(EmptyParams::default()),
    };
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"method\":\"session.snapshot\""));
    let restored: Request = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, request);

    let response = SuccessResponse {
        id: "req_snapshot".into(),
        result: ResponseResult::SessionSnapshot {
            snapshot: Box::new(SessionSnapshot {
                version: "0.1.2".into(),
                protocol: 16,
                focused_workspace_id: None,
                focused_tab_id: None,
                focused_pane_id: None,
                workspaces: Vec::new(),
                tabs: Vec::new(),
                panes: Vec::new(),
                layouts: Vec::new(),
                agents: Vec::new(),
            }),
        },
    };
    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"type\":\"session_snapshot\""));
    let restored: SuccessResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}

#[test]
fn layout_split_ratio_response_round_trips() {
    let response = SuccessResponse {
        id: "layout_ratio".into(),
        result: ResponseResult::LayoutSplitRatioSet {
            layout: LayoutDescription {
                workspace_id: "w1".into(),
                tab_id: "w1:1".into(),
                zoomed: false,
                focused_pane_id: "w1-1".into(),
                root: LayoutNode::Pane {
                    pane: LayoutPane {
                        pane_id: Some("w1-1".into()),
                        ..Default::default()
                    },
                },
            },
        },
    };
    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"type\":\"layout_split_ratio_set\""));
    let restored: SuccessResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}

#[test]
fn create_response_round_trips_with_root_pane() {
    let response = SuccessResponse {
        id: "req_2".into(),
        result: ResponseResult::TabCreated {
            tab: TabInfo {
                tab_id: "w_1:2".into(),
                workspace_id: "w_1".into(),
                number: 2,
                label: "review".into(),
                focused: false,
                pane_count: 1,
                agent_status: AgentStatus::Unknown,
            },
            root_pane: PaneInfo {
                pane_id: "w_1-3".into(),
                terminal_id: "term_example".into(),
                workspace_id: "w_1".into(),
                tab_id: "w_1:2".into(),
                focused: false,
                cwd: Some("/tmp/review".into()),
                foreground_cwd: None,
                label: None,
                agent: None,
                title: None,
                terminal_title: None,
                terminal_title_stripped: None,
                display_agent: None,
                agent_status: AgentStatus::Unknown,
                state_labels: HashMap::new(),
                tokens: HashMap::new(),
                agent_session: None,
                scroll: None,
                revision: 0,
            },
        },
    };

    let json = serde_json::to_string(&response).unwrap();
    assert!(json.contains("\"type\":\"tab_created\""));
    assert!(json.contains("\"root_pane\""));
    let restored: SuccessResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}

#[test]
fn error_response_round_trips() {
    let response = ErrorResponse {
        id: "req_1".into(),
        error: ErrorBody {
            code: "pane_not_found".into(),
            message: "pane p_1 not found".into(),
        },
    };

    let json = serde_json::to_string(&response).unwrap();
    let restored: ErrorResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, response);
}
