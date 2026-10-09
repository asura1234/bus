use super::*;

#[test]
fn agent_wait_probe_only_translates_agent_disappearance() {
    let disappeared = agent_wait_probe_error(ErrorResponse {
        id: "wait".into(),
        error: ErrorBody {
            code: "agent_not_found".into(),
            message: "missing".into(),
        },
    })
    .unwrap();
    let disappeared: ErrorResponse = serde_json::from_str(&disappeared).unwrap();
    assert_eq!(disappeared.id, "wait");
    assert_eq!(disappeared.error.code, "agent_not_running");

    let unavailable = agent_wait_probe_error(ErrorResponse {
        id: "wait".into(),
        error: ErrorBody {
            code: "server_unavailable".into(),
            message: "timed out waiting for app response".into(),
        },
    })
    .unwrap();
    let unavailable: ErrorResponse = serde_json::from_str(&unavailable).unwrap();
    assert_eq!(unavailable.id, "wait");
    assert_eq!(unavailable.error.code, "server_unavailable");
}

fn agent_wait_for_pane(pane_id: &str) -> ResolvedAgentWait {
    let initial: crate::protocol::api::schema::AgentInfo =
        serde_json::from_value(serde_json::json!({
            "terminal_id": "term_logs",
            "agent": "pi",
            "agent_status": "working",
            "workspace_id": "w_1",
            "tab_id": "t_1_2",
            "pane_id": pane_id,
            "focused": false,
            "revision": 0
        }))
        .unwrap();
    ResolvedAgentWait {
        target: pane_id.to_string(),
        until: agent_wait_statuses(Vec::new()),
        timeout_ms: None,
        initial,
        last_event_sequence: 0,
        after_state_change_seq: None,
        accept_transient_status: true,
        timeout_kind: AgentWaitTimeoutKind::Status,
    }
}

#[test]
fn agent_wait_ends_when_the_agent_tab_is_closed() {
    let event_hub = EventHub::default();
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = crate::server::app::App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        api_rx,
        event_hub.clone(),
    );
    app.state.workspaces = vec![crate::server::workspaces::Workspace::test_new("agents")];
    let tab_idx = app.state.workspaces[0].test_add_tab(Some("logs"));
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    let agent_pane = app.state.workspaces[0].tabs[tab_idx].root_pane;
    let public_pane_id = app.public_pane_id(0, agent_pane).unwrap();
    let tab_id = app.public_tab_id(0, tab_idx).unwrap();
    let wait = agent_wait_for_pane(&public_pane_id);
    let mut last_event_sequence = event_hub.current_sequence();

    let closed = app.handle_tab_close(
        "close".into(),
        crate::protocol::api::schema::TabTarget { tab_id },
    );
    assert!(closed.contains("\"ok\""), "tab close failed: {closed}");

    let scan = scan_agent_wait_events(
        "wait_req",
        &wait,
        &public_pane_id,
        &wait.initial.agent.clone(),
        &mut last_event_sequence,
        &event_hub,
    )
    .unwrap();
    assert!(
        matches!(
            scan,
            AgentWaitEventScan::Outcome(_)
                | AgentWaitEventScan::Probe {
                    should_probe: true,
                    ..
                }
        ),
        "closing the agent's tab published no event the agent wait reacts to"
    );
}

#[test]
fn agent_wait_probes_after_container_close_without_assuming_agent_exit() {
    for data in [
        EventData::TabClosed {
            tab_id: "other_tab".into(),
            workspace_id: "other_workspace".into(),
        },
        EventData::WorkspaceClosed {
            workspace_id: "w_1".into(),
            workspace: None,
        },
        EventData::WorkspaceClosed {
            workspace_id: "other_workspace".into(),
            workspace: None,
        },
    ] {
        let event_hub = EventHub::default();
        let event = match data {
            EventData::TabClosed { .. } => EventKind::TabClosed,
            _ => EventKind::WorkspaceClosed,
        };
        event_hub.push(EventEnvelope { event, data });
        let wait = agent_wait_for_pane("pane_1");
        let scan = scan_agent_wait_events(
            "wait_req",
            &wait,
            "pane_1",
            &wait.initial.agent,
            &mut 0,
            &event_hub,
        )
        .unwrap();
        assert!(
            matches!(
                scan,
                AgentWaitEventScan::Probe {
                    should_probe: true,
                    matched_event_status: None,
                }
            ),
            "a container close must recheck identity rather than assume agent exit"
        );
    }
}
