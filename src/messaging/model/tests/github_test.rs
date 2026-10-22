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
    state
        .create_agent(
            room,
            "worker",
            super::super::Provider::Codex,
            "/repo".into(),
            Some("fix/ours".into()),
        )
        .unwrap();
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

#[test]
fn github_watch_orchestrator_discovers_repos_without_contributing_owned_heads() {
    let (mut state, room, orchestrator) = bound_room();
    state.agents.get_mut(&orchestrator).unwrap().branch = Some("fix/orchestrator".into());
    state
        .create_agent(
            room,
            "member",
            super::super::Provider::ClaudeCode,
            "/member".into(),
            Some("fix/member".into()),
        )
        .unwrap();
    let interests = state.github_interests();
    assert!(interests[0]
        .agents
        .iter()
        .any(|(cwd, _)| cwd == &std::path::PathBuf::from("/repo")));
    let branches: Vec<_> = interests[0]
        .agents
        .iter()
        .filter_map(|(_, b)| b.as_deref())
        .collect();
    assert_eq!(
        branches,
        ["fix/member"],
        "orchestrator head became room ownership"
    );
}

#[test]
fn github_watch_merged_head_expires_and_is_not_remembered_again_after_restart() {
    let (mut state, room, _) = bound_room();
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
    state.apply_github(&coverage, 1).unwrap();
    state
        .apply_github(&merged_update(room, "fix/ours"), 2)
        .unwrap();
    assert!(
        state
            .room(room)
            .unwrap()
            .github_branches
            .get("o/r")
            .is_none_or(|b| !b.contains("fix/ours")),
        "merged head stayed remembered"
    );
    let mut state: BusState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    state.apply_github(&coverage, 3).unwrap();
    assert!(
        state
            .room(room)
            .unwrap()
            .github_branches
            .get("o/r")
            .is_none_or(|b| !b.contains("fix/ours")),
        "discovery re-remembered a terminal head"
    );
}

#[test]
fn github_watch_closed_head_expires_but_a_different_open_pr_keeps_the_head_owned() {
    let (mut state, room, _) = bound_room();
    let mut update = merged_update(room, "fix/ours");
    let open = PullRequest {
        number: 138,
        branch: "fix/ours".into(),
        base: "main".into(),
        ..Default::default()
    };
    if let Update::Repo { result, .. } = &mut update {
        *result = Ok(Page {
            default_branch: "main".into(),
            open: vec![open.clone()],
            ..Default::default()
        });
    }
    state.apply_github(&update, 1).unwrap();
    state
        .apply_github(&merged_update(room, "fix/ours"), 2)
        .unwrap();
    assert!(
        state.room(room).unwrap().github_branches["o/r"].contains("fix/ours"),
        "an older merged PR disowned the active PR on the same head"
    );
    let before = state.requests().count();
    if let Update::Repo { result, .. } = &mut update {
        *result = Ok(Page {
            default_branch: "main".into(),
            closed: vec![open.clone()],
            ..Default::default()
        });
    }
    state.apply_github(&update, 3).unwrap();
    assert!(!state.room(room).unwrap().github_branches["o/r"].contains("fix/ours"));
    assert!(!state.github_watermarks()["o/r"].observed[&room.0].contains_key(&138));
    assert_eq!(
        state.requests().count(),
        before,
        "closure must not become a merge notification"
    );
    if let Update::Repo { result, .. } = &mut update {
        *result = Ok(Page {
            default_branch: "main".into(),
            open: vec![open],
            ..Default::default()
        });
    }
    state.apply_github(&update, 4).unwrap();
    assert!(
        state.room(room).unwrap().github_branches["o/r"].contains("fix/ours"),
        "reopened PR could not restore current-member ownership"
    );
}

#[test]
fn github_watch_new_open_pr_can_reestablish_retired_head_ownership() {
    let (mut state, room, _) = bound_room();
    state
        .apply_github(&merged_update(room, "fix/ours"), 1)
        .unwrap();
    assert!(!state.room(room).unwrap().github_branches["o/r"].contains("fix/ours"));
    let mut update = merged_update(room, "fix/ours");
    if let Update::Repo { result, .. } = &mut update {
        *result = Ok(Page {
            default_branch: "main".into(),
            open: vec![PullRequest {
                number: 138,
                branch: "fix/ours".into(),
                base: "main".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
    }
    state.apply_github(&update, 2).unwrap();
    assert!(state.room(room).unwrap().github_branches["o/r"].contains("fix/ours"));
}

#[test]
fn github_watch_a_closed_pr_does_not_retire_a_head_whose_open_pr_is_on_a_later_page() {
    let (mut state, room, _) = bound_room();
    // The worker that opened fix/ours has been deleted; only the remembered
    // head links the room to its still-open PR #10.
    state
        .apply_github(
            &Update::Coverage(
                [(
                    "o/r".into(),
                    vec![Scope {
                        room: room.0,
                        branches: ["fix/ours".into()].into(),
                    }],
                )]
                .into(),
            ),
            1,
        )
        .unwrap();
    // The poller builds each scan's scope from the room's remembered heads.
    let page = |state: &BusState, result: Page| Update::Repo {
        repo: "o/r".into(),
        scopes: vec![Scope {
            room: room.0,
            branches: state.room(room).unwrap().github_branches["o/r"].clone(),
        }],
        result: Ok(result),
        at_ms: 2000,
    };
    let pr = |number| PullRequest {
        number,
        branch: "fix/ours".into(),
        base: "main".into(),
        ..Default::default()
    };
    // An earlier PR #9 from the same head was closed. More than one page of
    // open PRs exists, so PR #10 has not been scanned yet.
    let first = page(
        &state,
        Page {
            default_branch: "main".into(),
            closed: vec![pr(9)],
            open_cursor: Some("next".into()),
            ..Default::default()
        },
    );
    state.apply_github(&first, 2).unwrap();
    let second = page(
        &state,
        Page {
            default_branch: "main".into(),
            open: vec![pr(10)],
            ..Default::default()
        },
    );
    state.apply_github(&second, 3).unwrap();
    assert!(
        state.room(room).unwrap().github_branches["o/r"].contains("fix/ours"),
        "a closed PR seen before its head's open PR permanently disowned the open PR"
    );
}

#[test]
fn github_watch_an_orchestrator_head_saved_before_the_exclusion_stops_owning_prs() {
    let (mut state, room, orchestrator) = bound_room();
    state.agents.get_mut(&orchestrator).unwrap().branch = Some("fix/orchestrator".into());
    // Saved by a Bus that still remembered the orchestrator's checkout branch.
    state
        .rooms
        .get_mut(&room)
        .unwrap()
        .github_branches
        .entry("o/r".into())
        .or_default()
        .insert("fix/orchestrator".into());
    let interests = state.github_interests();
    let coverage = Update::Coverage(
        [(
            "o/r".into(),
            vec![Scope {
                room: room.0,
                branches: interests[0]
                    .agents
                    .iter()
                    .filter_map(|(_, branch)| branch.clone())
                    .collect(),
            }],
        )]
        .into(),
    );
    state.apply_github(&coverage, 1).unwrap();
    assert!(
        !state.room(room).unwrap().github_branches["o/r"].contains("fix/orchestrator"),
        "an orchestrator head remembered by an older Bus still establishes ownership"
    );
}

fn remembered_scope(state: &BusState, room: RoomId) -> Vec<Scope> {
    let interest = state
        .github_interests()
        .into_iter()
        .find(|interest| interest.room == room.0)
        .unwrap();
    vec![Scope {
        room: room.0,
        branches: interest
            .owned_branches
            .get("o/r")
            .cloned()
            .unwrap_or_default(),
    }]
}

#[test]
fn github_watch_reopened_pr_restores_ownership_after_worker_cleanup_and_restart() {
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
    state
        .apply_github(
            &Update::Coverage(
                [(
                    "o/r".into(),
                    vec![Scope {
                        room: room.0,
                        branches: ["fix/ours".into()].into(),
                    }],
                )]
                .into(),
            ),
            1,
        )
        .unwrap();
    state.delete_agent(worker, 2).unwrap();
    let mut pr = PullRequest {
        number: 9,
        branch: "fix/ours".into(),
        base: "main".into(),
        ..Default::default()
    };
    state
        .apply_github(
            &Update::Repo {
                repo: "o/r".into(),
                scopes: remembered_scope(&state, room),
                result: Ok(Page {
                    closed: vec![pr.clone()],
                    ..Default::default()
                }),
                at_ms: 3,
            },
            3,
        )
        .unwrap();
    assert!(
        remembered_scope(&state, room)[0].branches.is_empty(),
        "closed head still owns PRs"
    );
    let mut state: BusState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    for at in [4, 5] {
        pr.status.ready = at == 5;
        state
            .apply_github(
                &Update::Repo {
                    repo: "o/r".into(),
                    scopes: remembered_scope(&state, room),
                    result: Ok(Page {
                        open: vec![pr.clone()],
                        ..Default::default()
                    }),
                    at_ms: at,
                },
                at,
            )
            .unwrap();
    }
    assert!(
        remembered_scope(&state, room)[0]
            .branches
            .contains("fix/ours"),
        "reopened head lost ownership after worker cleanup"
    );
    assert!(
        state
            .requests()
            .any(|request| request.agent_id == orchestrator
                && request.prompt.text.contains("#9 ready for review")),
        "reopened PR's transition did not reach its orchestrator"
    );
}

#[test]
fn github_watch_terminal_observation_mid_scan_waits_for_the_next_full_open_pass() {
    let (mut state, room, _) = bound_room();
    state
        .apply_github(
            &Update::Coverage(
                [(
                    "o/r".into(),
                    vec![Scope {
                        room: room.0,
                        branches: ["fix/ours".into()].into(),
                    }],
                )]
                .into(),
            ),
            1,
        )
        .unwrap();
    let pages = [
        Page {
            open_cursor: Some("page-2".into()),
            ..Default::default()
        },
        Page {
            closed: vec![PullRequest {
                number: 9,
                branch: "fix/ours".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
        Page {
            open_cursor: Some("page-2".into()),
            ..Default::default()
        },
        Page::default(),
    ];
    for (index, page) in pages.into_iter().enumerate() {
        state
            .apply_github(
                &Update::Repo {
                    repo: "o/r".into(),
                    scopes: remembered_scope(&state, room),
                    result: Ok(page),
                    at_ms: index as u64 + 2,
                },
                index as u64 + 2,
            )
            .unwrap();
        assert_eq!(
            remembered_scope(&state, room)[0]
                .branches
                .contains("fix/ours"),
            index < 3,
            "retirement must follow a full pass begun since the terminal observation"
        );
        let restarted = serde_json::to_string(&state).unwrap();
        state = serde_json::from_str(&restarted).unwrap();
    }
}

#[test]
fn github_watch_orchestrator_head_shared_by_a_current_member_keeps_ownership() {
    let (mut state, room, orchestrator) = bound_room();
    state.agents.get_mut(&orchestrator).unwrap().branch = Some("fix/shared".into());
    state
        .create_agent(
            room,
            "worker",
            super::super::Provider::Codex,
            "/repo".into(),
            Some("fix/shared".into()),
        )
        .unwrap();
    state
        .apply_github(
            &Update::Coverage(
                [(
                    "o/r".into(),
                    vec![Scope {
                        room: room.0,
                        branches: ["fix/shared".into()].into(),
                    }],
                )]
                .into(),
            ),
            1,
        )
        .unwrap();
    assert!(remembered_scope(&state, room)[0]
        .branches
        .contains("fix/shared"));
    assert!(state.github_interests()[0]
        .agents
        .iter()
        .any(|(_, branch)| branch.as_deref() == Some("fix/shared")));
}
