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
    assert!(!requests[0].prompt.text.contains(" @"));
    assert_eq!(requests[0].agent_id, agent);
    let mut state: BusState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    state.apply_github(&update, 3000).unwrap();
    assert_eq!(state.requests().count(), 1);
    // Without an orchestrator, informational notices must also survive restart.
    let mut state = BusState::new();
    let room = state.create_room("unbound").unwrap();
    state
        .github_notice(room, false, "GitHub: o/r #137 merged into main".into(), 1)
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

fn bound_room() -> (BusState, RoomId, crate::messaging::model::AgentId) {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let room = state.create_room("work").unwrap();
    let orchestrator = state
        .create_agent(
            master,
            "libtv-orch",
            super::super::Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orchestrator, room).unwrap();
    (state, room, orchestrator)
}

fn merged_update(room: RoomId, branch: &str) -> Update {
    Update::Repo {
        repo: "o/r".into(),
        scopes: vec![Scope {
            room: room.0,
            branches: ["fix/ours".into()].into(),
        }],
        result: Ok(Page {
            default_branch: "main".into(),
            merged: vec![PullRequest {
                number: 137,
                branch: branch.into(),
                base: "main".into(),
                merged_at_ms: Some(1000),
                ..Default::default()
            }],
            ..Default::default()
        }),
        at_ms: 2000,
    }
}

#[test]
fn github_watch_identifies_orchestrator_through_recipient_without_a_suffix() {
    let (mut state, room, orchestrator) = bound_room();
    state
        .apply_github(&merged_update(room, "fix/ours"), 2000)
        .unwrap();
    let request = state.requests().next().unwrap();
    assert_eq!(request.agent_id, orchestrator);
    assert!(request
        .prompt
        .text
        .ends_with("Next: cleanup / verify / follow-up."));
    assert!(!request.prompt.text.contains(" @"));
}

#[test]
fn github_watch_informational_merge_batch_does_not_wake_the_orchestrator() {
    let (mut state, room, _) = bound_room();
    state
        .apply_github(&merged_update(room, "fix/theirs"), 2000)
        .unwrap();
    assert_eq!(
        state.notices(room).count(),
        1,
        "informational batch must remain visible"
    );
    assert_eq!(
        state.requests().count(),
        0,
        "informational batch was delivered: {:?}",
        state.requests().map(|r| &r.prompt.text).collect::<Vec<_>>()
    );
}

#[test]
fn github_watch_recovery_rearms_a_new_outage_after_restart() {
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let mut update = merged_update(room, "fix/theirs");
    let failure = crate::messaging::github::source::Failure {
        reason: "not logged in",
        retry_at_ms: None,
    };
    if let Update::Repo { result, .. } = &mut update {
        *result = Err(failure.clone());
    }
    state.apply_github(&update, 1).unwrap();
    state.apply_github(&update, 2).unwrap();
    assert_eq!(state.notices(room).count(), 1);
    if let Update::Repo { result, .. } = &mut update {
        *result = Ok(Page::default());
    }
    state.apply_github(&update, 3).unwrap();
    let mut state: BusState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    if let Update::Repo { result, .. } = &mut update {
        *result = Err(failure);
    }
    state.apply_github(&update, 4).unwrap();
    state.apply_github(&update, 5).unwrap();
    assert_eq!(
        state.notices(room).count(),
        2,
        "new outage was suppressed after recovery"
    );
}

#[test]
fn github_watch_one_failing_repo_beside_a_healthy_repo_reports_its_outage_once() {
    let (mut state, room, _) = bound_room();
    let scope = || {
        vec![Scope {
            room: room.0,
            branches: ["fix/ours".into()].into(),
        }]
    };
    let failed = Update::Repo {
        repo: "o/gone".into(),
        scopes: scope(),
        result: Err(crate::messaging::github::source::Failure {
            reason: "GitHub repository unavailable",
            retry_at_ms: None,
        }),
        at_ms: 1,
    };
    let healthy = Update::Repo {
        repo: "o/r".into(),
        scopes: scope(),
        result: Ok(Page {
            default_branch: "main".into(),
            ..Default::default()
        }),
        at_ms: 1,
    };
    for round in 0..3 {
        state.apply_github(&failed, 10 + round).unwrap();
        state.apply_github(&healthy, 10 + round).unwrap();
    }
    let outages = state
        .requests()
        .filter(|r| r.prompt.text.contains("watch unavailable"))
        .count();
    assert_eq!(
        outages, 1,
        "the same o/gone outage was announced every round"
    );
}

#[test]
fn github_watch_remembers_owned_heads_after_worker_deletion_and_restart() {
    let (mut state, room, orchestrator) = bound_room();
    let worker = state
        .create_agent(
            room,
            "worker",
            super::super::Provider::ClaudeCode,
            "/repo".into(),
            Some("fix/ours".into()),
        )
        .unwrap();
    let coverage = Update::Coverage(
        [(
            "o/r".into(),
            vec![Scope {
                room: room.0,
                branches: ["fix/ours".into()].into(),
            }],
        )]
        .into(),
    );
    let before = state.clone();
    state.apply_github(&coverage, 1).unwrap();
    assert!(state.durable_poll_change_from(&before));
    state.delete_agent(worker, 2).unwrap();
    let mut state: BusState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    let interests = state.github_interests();
    assert_eq!(interests[0].remembered, ["o/r".into()].into());
    assert_eq!(
        interests[0].owned_branches["o/r"],
        ["fix/ours".into()].into()
    );
    assert!(interests[0]
        .agents
        .iter()
        .all(|(_, branch)| branch.is_none()));
    let scopes = vec![Scope {
        room: room.0,
        branches: interests[0].owned_branches["o/r"].clone(),
    }];
    let mut update = merged_update(room, "fix/ours");
    if let Update::Repo { scopes: target, .. } = &mut update {
        *target = scopes;
    }
    state.apply_github(&update, 3).unwrap();
    let requests: Vec<_> = state.requests().collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id, orchestrator);
    assert!(requests[0].prompt.text.contains("#137 merged"));
    state
        .set_github_repos(room, &serde_json::json!([]))
        .unwrap();
    state
        .apply_github(
            &Update::Coverage(
                [(
                    "other/repo".into(),
                    vec![Scope {
                        room: room.0,
                        branches: ["unwanted".into()].into(),
                    }],
                )]
                .into(),
            ),
            4,
        )
        .unwrap();
    assert!(!state
        .room(room)
        .unwrap()
        .github_branches
        .contains_key("other/repo"));
}

#[test]
fn github_watch_outages_recover_per_repo_and_persist_per_room() {
    let mut state = BusState::new();
    let a = state.create_room("a").unwrap();
    let b = state.create_room("b").unwrap();
    let update = |room: RoomId, repo: &str, failed: bool| Update::Repo {
        repo: repo.into(),
        scopes: vec![Scope {
            room: room.0,
            ..Default::default()
        }],
        result: if failed {
            Err(crate::messaging::github::source::Failure {
                reason: "not logged in",
                retry_at_ms: None,
            })
        } else {
            Ok(Page::default())
        },
        at_ms: 1,
    };
    state.apply_github(&update(a, "o/one", true), 1).unwrap();
    state.apply_github(&update(a, "o/two", true), 2).unwrap();
    state.apply_github(&update(b, "o/one", true), 3).unwrap();
    assert_eq!(state.notices(a).count(), 2);
    assert_eq!(state.notices(b).count(), 1);
    assert!(state
        .notices(a)
        .any(|n| n.text.contains("GitHub: o/one watch unavailable")));
    let mut state: BusState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    state.apply_github(&update(a, "o/one", false), 4).unwrap();
    state.apply_github(&update(a, "o/two", true), 5).unwrap();
    state.apply_github(&update(b, "o/one", true), 6).unwrap();
    assert_eq!(state.notices(a).count(), 2, "a different repo was rearmed");
    assert_eq!(state.notices(b).count(), 1, "a different room was rearmed");
    state.apply_github(&update(a, "o/one", true), 7).unwrap();
    state.apply_github(&update(a, "o/one", true), 8).unwrap();
    assert_eq!(
        state.notices(a).count(),
        3,
        "the recovered repo must report one new outage"
    );
}
