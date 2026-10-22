use super::*;
use crate::messaging::github::{Page, PullRequest, Scope};
use crate::messaging::model::Author;
#[test]
fn github_watch_notices_and_watermarks_survive_restart_and_keep_normal_bus_delivery() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            master,
            "orch",
            super::super::Provider::Codex,
            "/repo".into(),
            Some("fix/ours".into()),
        )
        .unwrap();
    state.bind_orchestrator(agent, room).unwrap();
    let page = Page {
        default_branch: "main".into(),
        merged: vec![PullRequest {
            number: 137,
            branch: "fix/ours".into(),
            base: "main".into(),
            merged_at_ms: Some(1000),
            ..Default::default()
        }],
        ..Default::default()
    };
    let update = Update::Repo {
        repo: "o/r".into(),
        scopes: vec![Scope {
            room: room.0,
            branches: ["fix/ours".into()].into(),
        }],
        result: Ok(page),
        at_ms: 2000,
    };
    let before = state.clone();
    state.apply_github(&update, 2000).unwrap();
    assert!(state.durable_poll_change_from(&before));
    let requests: Vec<_> = state.requests().collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt.author, Author::Bus);
    assert!(requests[0].prompt.text.contains(&format!("@{}", agent.0)));
    assert_eq!(requests[0].agent_id, agent);
    let mut state: BusState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    state.apply_github(&update, 3000).unwrap();
    assert_eq!(state.requests().count(), 1);
    // Without an orchestrator, informational notices must also survive restart.
    let mut state = BusState::new();
    let room = state.create_room("unbound").unwrap();
    state
        .github_notice(room, "GitHub: o/r #137 merged into main".into(), 1)
        .unwrap();
    let state: BusState = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert_eq!(state.notices(room).count(), 1);
}
#[test]
fn github_watch_reports_unavailable_once_per_room_even_after_restart_and_validates_settings() {
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let master = state.ensure_master_room();
    let update = Update::Repo {
        repo: "o/r".into(),
        scopes: vec![Scope {
            room: room.0,
            ..Default::default()
        }],
        result: Err(crate::messaging::github::source::Failure {
            reason: "not logged in",
            retry_at_ms: None,
        }),
        at_ms: 1,
    };
    state.apply_github(&update, 1).unwrap();
    let mut state: BusState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    state.apply_github(&update, 2).unwrap();
    assert_eq!(state.notices(room).count(), 1);
    state
        .set_github_repos(room, &serde_json::json!(["Owner/Repo"]))
        .unwrap();
    assert_eq!(
        state.room(room).unwrap().github_repos,
        Some(vec!["owner/repo".into()])
    );
    assert!(state
        .set_github_repos(room, &serde_json::json!(["https://gitlab.com/o/r"]))
        .is_err());
    assert!(state
        .set_github_repos(master, &serde_json::Value::Null)
        .is_err());
    state
        .set_github_repos(room, &serde_json::json!([]))
        .unwrap();
    assert_eq!(state.github_interests()[0].repos, Some(vec![]));
}
