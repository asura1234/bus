//! Agent availability as the pane listing reports it.

use super::*;

/// One `agent.list` answer for the fixture's pane: the provider identified
/// as `agent` (None once the pane runs something else) with its screen
/// status, or no entry at all once the pane is no longer an agent.
fn pane_listing(agent: Option<&str>, status: &str) -> Result<ResponseResult, TransportError> {
    let info: crate::api::schema::AgentInfo = serde_json::from_value(json!({
        "terminal_id": "terminal", "pane_id": "pane", "agent": agent,
        "agent_status": status, "agent_session": agent.map(|agent| json!({
            "source": format!("herdr:{agent}"), "agent": agent, "kind": "id", "value": "session"
        })),
        "workspace_id": "w1", "tab_id": "t1", "focused": false,
        "interactive_ready": true, "revision": 1
    }))
    .unwrap();
    Ok(ResponseResult::AgentList { agents: vec![info] })
}

#[test]
fn an_agent_whose_provider_is_not_running_is_unavailable_until_it_returns() {
    let no_agents = Ok(ResponseResult::AgentList { agents: Vec::new() });
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![
            pane_listing(Some("codex"), "idle"),
            // Codex's own updater replaced its UI: the screen is not Codex.
            pane_listing(Some("codex"), "unknown"),
            // The provider exited and the shell is back.
            no_agents,
            // Another program holds the pane.
            pane_listing(None, "unknown"),
            // Relaunched.
            pane_listing(Some("codex"), "idle"),
        ],
    );
    let status = |worker: &Worker| worker.state.agent(agent).unwrap().status;
    worker.poll().unwrap();
    assert_eq!(status(&worker), RuntimeStatus::Idle);
    let request = queue(&mut worker, room, agent, "held while unavailable");
    for case in ["updater", "exited", "foreign program"] {
        worker.poll().unwrap();
        assert_eq!(status(&worker), RuntimeStatus::Unavailable, "{case}");
        assert_eq!(
            worker.state.room_status(room),
            RuntimeStatus::Blocked,
            "{case}"
        );
        assert_eq!(
            crate::bus::diagnostics::wait_reason(worker.state.agent(agent).unwrap()),
            Some("agent_unavailable"),
            "{case}"
        );
        worker.submit_ready().unwrap();
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Queued,
            "{case}: the delivery stays queued"
        );
    }
    assert!(!calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.starts_with("agent.prompt")));
    worker.poll().unwrap();
    assert_eq!(status(&worker), RuntimeStatus::Idle);
    assert_eq!(worker.state.room_status(room), RuntimeStatus::Idle);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
