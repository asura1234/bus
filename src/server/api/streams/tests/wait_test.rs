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

fn spawn_pane_not_found_app() -> (ApiRequestSender, std::thread::JoinHandle<()>) {
    let (api_tx, mut api_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::server::api::ApiRequestMessage>();
    let app = std::thread::spawn(move || {
        while let Some(msg) = api_rx.blocking_recv() {
            let response = serde_json::to_string(&ErrorResponse {
                id: msg.request.id,
                error: ErrorBody {
                    code: "pane_not_found".into(),
                    message: "pane pane_gone not found".into(),
                },
            })
            .unwrap();
            msg.respond_to.send(response).unwrap();
        }
    });
    (api_tx, app)
}

#[test]
fn agent_status_subscription_setup_error_carries_the_client_request_id() {
    let (api_tx, app) = spawn_pane_not_found_app();
    let event_hub = EventHub::default();

    let Err(error) = ActiveSubscription::new(
        Subscription::PaneAgentStatusChanged {
            pane_id: "pane_gone".into(),
            agent_status: None,
        },
        "sub_req",
        0,
        &api_tx,
        &event_hub,
        event_hub.current_sequence(),
    ) else {
        panic!("subscription to a missing pane must fail");
    };

    assert_eq!(error.error.code, "pane_not_found");
    assert_eq!(error.id, "sub_req");
    drop(api_tx);
    app.join().unwrap();
}

#[test]
fn output_matched_subscription_setup_error_carries_the_client_request_id() {
    let (api_tx, app) = spawn_pane_not_found_app();
    let event_hub = EventHub::default();

    let Err(error) = ActiveSubscription::new(
        Subscription::PaneOutputMatched {
            pane_id: "pane_gone".into(),
            source: crate::protocol::api::schema::ReadSource::Recent,
            lines: None,
            r#match: crate::protocol::api::schema::OutputMatch::Substring {
                value: "ready".into(),
            },
            strip_ansi: true,
        },
        "sub_req",
        0,
        &api_tx,
        &event_hub,
        event_hub.current_sequence(),
    ) else {
        panic!("subscription to a missing pane must fail");
    };

    assert_eq!(error.error.code, "internal_error");
    assert_eq!(error.error.message, "failed to decode pane read error");
    assert_eq!(error.id, "sub_req");
    drop(api_tx);
    app.join().unwrap();
}

#[test]
fn events_wait_on_missing_pane_answers_with_the_client_request_id() {
    let (api_tx, app) = spawn_pane_not_found_app();
    let event_hub = EventHub::default();
    let path = std::env::temp_dir().join(format!(
        "bus-events-wait-missing-pane-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let listener = crate::platform::ipc::bind_local_listener(&path).unwrap();
    let _client = crate::platform::ipc::connect_local_stream(&path).unwrap();
    let mut server = {
        use interprocess::local_socket::traits::Listener as _;
        listener.accept().unwrap()
    };
    let running = Arc::new(AtomicBool::new(true));

    let response = wait_for_event(
        "wait_req".into(),
        EventsWaitParams {
            match_event: EventMatch::PaneAgentStatusChanged {
                pane_id: "pane_gone".into(),
                agent_status: crate::protocol::api::schema::AgentStatus::Done,
            },
            timeout_ms: Some(200),
        },
        &mut server,
        &api_tx,
        &event_hub,
        &running,
    )
    .unwrap()
    .expect("events.wait answers a missing pane");
    let response: ErrorResponse = serde_json::from_str(&response).unwrap();

    assert_eq!(response.error.code, "pane_not_found");
    assert_eq!(response.id, "wait_req");
    drop(api_tx);
    app.join().unwrap();
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
fn scroll_subscription_setup_error_carries_the_client_request_id() {
    let (api_tx, app) = spawn_pane_not_found_app();
    let event_hub = EventHub::default();
    let Err(error) = ActiveSubscription::new(
        Subscription::PaneScrollChanged {
            pane_id: "pane_gone".into(),
        },
        "scroll_req",
        1,
        &api_tx,
        &event_hub,
        event_hub.current_sequence(),
    ) else {
        panic!("subscription to a missing pane must fail");
    };
    assert_eq!(error.error.code, "pane_not_found");
    assert_eq!(error.error.message, "pane pane_gone not found");
    assert_eq!(error.id, "scroll_req");
    drop(api_tx);
    app.join().unwrap();
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
