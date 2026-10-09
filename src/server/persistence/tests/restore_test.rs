use super::*;

fn test_session_path(name: &str) -> String {
    std::env::current_dir()
        .unwrap()
        .join(name)
        .display()
        .to_string()
}

#[test]
fn capture_and_restore_node_round_trip() {
    let node = Node::Split {
        direction: Direction::Horizontal,
        ratio: 0.5,
        first: Box::new(Node::Pane(PaneId::from_raw(0))),
        second: Box::new(Node::Split {
            direction: Direction::Vertical,
            ratio: 0.3,
            first: Box::new(Node::Pane(PaneId::from_raw(1))),
            second: Box::new(Node::Pane(PaneId::from_raw(2))),
        }),
    };

    let snap = super::super::schema::capture_node(&node);
    let (restored, id_map) = restore_node_remapped(&snap);

    assert_eq!(id_map.len(), 3);
    let ids = collect_pane_ids(&restored);
    assert_eq!(ids.len(), 3);
    let unique: std::collections::HashSet<u32> = ids.iter().map(|id| id.raw()).collect();
    assert_eq!(unique.len(), 3);
}

#[test]
fn prune_restored_node_collapses_missing_branch() {
    let keep = PaneId::from_raw(11);
    let missing = PaneId::from_raw(12);
    let node = Node::Split {
        direction: Direction::Horizontal,
        ratio: 0.5,
        first: Box::new(Node::Pane(keep)),
        second: Box::new(Node::Pane(missing)),
    };
    let surviving = std::collections::HashSet::from([keep]);

    let pruned = prune_restored_node(node, &surviving).expect("remaining pane should survive");

    assert!(matches!(pruned, Node::Pane(id) if id == keep));
}

#[test]
fn resolve_restored_pane_prefers_surviving_saved_id_and_falls_back_to_first_remaining() {
    let first = PaneId::from_raw(21);
    let second = PaneId::from_raw(22);
    let id_map = HashMap::from([(0_u32, first), (1_u32, second)]);
    let surviving = std::collections::HashSet::from([first]);
    let pane_ids = vec![first];

    assert_eq!(
        resolve_restored_pane(Some(0), &id_map, &surviving, &pane_ids),
        Some(first)
    );
    assert_eq!(
        resolve_restored_pane(Some(1), &id_map, &surviving, &pane_ids),
        Some(first)
    );
}

#[test]
fn restore_plan_respects_opt_in_and_allowlist() {
    let pi_session_path = test_session_path("pi-session.jsonl");
    let session = super::super::schema::PaneAgentSessionSnapshot {
        source: "herdr:pi".into(),
        agent: "pi".into(),
        kind: crate::agents::resume::catalog::AgentSessionRefKind::Path,
        value: pi_session_path.clone(),
    };

    assert!(restore_plan_for_snapshot(&session, false).is_none());
    assert_eq!(
        restore_plan_for_snapshot(&session, true).unwrap().argv,
        vec!["pi", "--session", pi_session_path.as_str()]
    );

    let unsupported_path = super::super::schema::PaneAgentSessionSnapshot {
        source: "herdr:claude".into(),
        agent: "claude".into(),
        kind: crate::agents::resume::catalog::AgentSessionRefKind::Path,
        value: test_session_path("claude-session"),
    };
    assert!(restore_plan_for_snapshot(&unsupported_path, true).is_none());
}

#[test]
fn restore_plan_selection_suppresses_duplicates() {
    let pi_session_path = test_session_path("pi-session.jsonl");
    let session = super::super::schema::PaneAgentSessionSnapshot {
        source: "herdr:pi".into(),
        agent: "pi".into(),
        kind: crate::agents::resume::catalog::AgentSessionRefKind::Path,
        value: pi_session_path.clone(),
    };
    let mut resumed = HashSet::new();
    let mut agent_restore = AgentRestoreState {
        enabled: false,
        resumed_sessions: &mut resumed,
    };

    assert!(
        pane_restore_startup(Some(&session), None, &mut agent_restore)
            .restore_plan
            .is_none()
    );
    assert!(agent_restore.resumed_sessions.is_empty());

    agent_restore.enabled = true;
    let first = pane_restore_startup(Some(&session), None, &mut agent_restore)
        .restore_plan
        .expect("first restore should get a plan");
    assert_eq!(
        first.argv,
        vec!["pi", "--session", pi_session_path.as_str()]
    );
    assert!(
        pane_restore_startup(Some(&session), None, &mut agent_restore)
            .restore_plan
            .is_none()
    );
}

#[test]
fn pane_restore_startup_suppresses_history_for_native_agent_resume() {
    let session = super::super::schema::PaneAgentSessionSnapshot {
        source: "herdr:pi".into(),
        agent: "pi".into(),
        kind: crate::agents::resume::catalog::AgentSessionRefKind::Path,
        value: test_session_path("pi-session.jsonl"),
    };
    let history = super::super::schema::PaneHistorySnapshot {
        ansi: "RESTORED_HISTORY\r\n".into(),
        lines: 1,
    };
    let mut resumed = HashSet::new();
    let mut agent_restore = AgentRestoreState {
        enabled: true,
        resumed_sessions: &mut resumed,
    };

    let startup = pane_restore_startup(Some(&session), Some(&history), &mut agent_restore);

    assert!(startup.restore_plan.is_some());
    assert!(startup.initial_history_ansi.is_none());
    assert!(!startup.duplicate_agent_session);
}

#[test]
fn pane_restore_startup_suppresses_history_for_duplicate_native_agent_session() {
    let session = super::super::schema::PaneAgentSessionSnapshot {
        source: "herdr:pi".into(),
        agent: "pi".into(),
        kind: crate::agents::resume::catalog::AgentSessionRefKind::Path,
        value: test_session_path("pi-session.jsonl"),
    };
    let history = super::super::schema::PaneHistorySnapshot {
        ansi: "RESTORED_HISTORY\r\n".into(),
        lines: 1,
    };
    let mut resumed = HashSet::new();
    let mut agent_restore = AgentRestoreState {
        enabled: true,
        resumed_sessions: &mut resumed,
    };

    let first = pane_restore_startup(Some(&session), Some(&history), &mut agent_restore);
    let duplicate = pane_restore_startup(Some(&session), Some(&history), &mut agent_restore);

    assert!(first.restore_plan.is_some());
    assert!(first.initial_history_ansi.is_none());
    assert!(duplicate.restore_plan.is_none());
    assert!(duplicate.initial_history_ansi.is_none());
    assert!(duplicate.duplicate_agent_session);
}

#[test]
fn pane_restore_startup_keeps_history_without_native_agent_resume() {
    let session = super::super::schema::PaneAgentSessionSnapshot {
        source: "herdr:pi".into(),
        agent: "pi".into(),
        kind: crate::agents::resume::catalog::AgentSessionRefKind::Path,
        value: test_session_path("pi-session.jsonl"),
    };
    let history = super::super::schema::PaneHistorySnapshot {
        ansi: "RESTORED_HISTORY\r\n".into(),
        lines: 1,
    };
    let mut resumed = HashSet::new();
    let mut agent_restore = AgentRestoreState {
        enabled: false,
        resumed_sessions: &mut resumed,
    };

    let startup = pane_restore_startup(Some(&session), Some(&history), &mut agent_restore);

    assert!(startup.restore_plan.is_none());
    assert_eq!(startup.initial_history_ansi, Some("RESTORED_HISTORY\r\n"));
    assert!(!startup.duplicate_agent_session);
    assert!(resumed.is_empty());
}

#[test]
fn restore_rehydrates_agent_session_metadata() {
    let session = super::super::schema::PaneAgentSessionSnapshot {
        source: "herdr:hermes".into(),
        agent: "hermes".into(),
        kind: crate::agents::resume::catalog::AgentSessionRefKind::Id,
        value: "hermes-session".into(),
    };

    let preserved = restored_terminal_agent_session(Some(&session), false)
        .expect("restore should preserve metadata");
    assert_eq!(preserved.source, "herdr:hermes");
    assert_eq!(preserved.agent, "hermes");
    assert_eq!(preserved.session_ref.value, "hermes-session");
}

#[test]
fn restore_does_not_rehydrate_duplicate_agent_session_metadata() {
    let session = super::super::schema::PaneAgentSessionSnapshot {
        source: "herdr:pi".into(),
        agent: "pi".into(),
        kind: crate::agents::resume::catalog::AgentSessionRefKind::Path,
        value: test_session_path("pi-session.jsonl"),
    };
    let mut resumed = HashSet::new();
    let mut agent_restore = AgentRestoreState {
        enabled: true,
        resumed_sessions: &mut resumed,
    };
    let first = pane_restore_startup(Some(&session), None, &mut agent_restore);
    let duplicate = pane_restore_startup(Some(&session), None, &mut agent_restore);
    assert!(first.restore_plan.is_some());
    assert!(duplicate.restore_plan.is_none());

    assert!(
        restored_terminal_agent_session(Some(&session), duplicate.duplicate_agent_session)
            .is_none()
    );
}

#[tokio::test]
async fn restore_carries_persisted_agent_session_metadata() {
    let cwd = std::env::current_dir().unwrap();
    let snapshot = SessionSnapshot {
        version: super::super::schema::SNAPSHOT_VERSION,
        workspaces: vec![WorkspaceSnapshot {
            id: Some("workspace".into()),
            custom_name: None,
            identity_cwd: cwd.clone(),
            public_pane_numbers: HashMap::new(),
            next_public_pane_number: 0,
            public_tab_numbers: Vec::new(),
            next_public_tab_number: 0,
            tabs: vec![TabSnapshot {
                custom_name: None,
                layout: LayoutSnapshot::Pane(0),
                panes: HashMap::from([(
                    0,
                    super::super::schema::PaneSnapshot {
                        cwd,
                        label: Some("reviewer".into()),
                        agent_name: Some("reviewer".into()),
                        managed_agent_kind: Some("opencode".into()),
                        agent_session: Some(super::super::schema::PaneAgentSessionSnapshot {
                            source: "herdr:opencode".into(),
                            agent: "opencode".into(),
                            kind: crate::agents::resume::catalog::AgentSessionRefKind::Id,
                            value: "opencode-session".into(),
                        }),
                        launch_argv: None,
                    },
                )]),
                zoomed: false,
                focused: Some(0),
                root_pane: Some(0),
            }],
            active_tab: 0,
        }],
        active: Some(0),
        selected: 0,
    };
    let (events, _event_rx) = mpsc::channel(4);

    let restored = restore(
        &snapshot,
        None,
        false,
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    );
    let RestoredSession {
        workspaces: _workspaces,
        terminals,
        launches: _runtimes,
        ..
    } = restored;

    let terminal = terminals
        .values()
        .next()
        .expect("restored terminal should exist");
    assert!(
        !terminal.respawn_shell_on_exit,
        "agent sessions should not use native restore lifecycle when resume_agents_on_restore is disabled"
    );
    assert_eq!(terminal.agent_name, None);
    assert_eq!(terminal.manual_label.as_deref(), Some("reviewer"));
    let session = terminal
        .persisted_agent_session
        .as_ref()
        .expect("persisted agent session should survive restore");
    assert_eq!(session.source, "herdr:opencode");
    assert_eq!(session.agent, "opencode");
    assert_eq!(session.session_ref.value, "opencode-session");
}

#[tokio::test]
async fn restore_preserves_public_id_mapping_after_pane_id_remap() {
    let cwd = std::env::current_dir().unwrap();
    let snapshot = SessionSnapshot {
        version: super::super::schema::SNAPSHOT_VERSION,
        workspaces: vec![WorkspaceSnapshot {
            id: Some("w1".into()),
            custom_name: None,
            identity_cwd: cwd.clone(),
            public_pane_numbers: HashMap::from([(10, 1), (20, 3)]),
            next_public_pane_number: 4,
            public_tab_numbers: vec![5],
            next_public_tab_number: 6,
            tabs: vec![TabSnapshot {
                custom_name: None,
                layout: LayoutSnapshot::Split {
                    direction: super::super::schema::DirectionSnapshot::Horizontal,
                    ratio: 0.5,
                    first: Box::new(LayoutSnapshot::Pane(10)),
                    second: Box::new(LayoutSnapshot::Pane(20)),
                },
                panes: HashMap::from([
                    (
                        10,
                        super::super::schema::PaneSnapshot {
                            cwd: cwd.clone(),
                            label: None,
                            agent_name: None,
                            managed_agent_kind: None,
                            agent_session: None,
                            launch_argv: None,
                        },
                    ),
                    (
                        20,
                        super::super::schema::PaneSnapshot {
                            cwd: cwd.clone(),
                            label: None,
                            agent_name: None,
                            managed_agent_kind: None,
                            agent_session: None,
                            launch_argv: None,
                        },
                    ),
                ]),
                zoomed: false,
                focused: Some(10),
                root_pane: Some(10),
            }],
            active_tab: 0,
        }],
        active: Some(0),
        selected: 0,
    };
    let (events, _event_rx) = mpsc::channel(4);

    let restored = restore(
        &snapshot,
        None,
        false,
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    );
    let RestoredSession {
        workspaces,
        terminals: _terminals,
        launches: _runtimes,
        ..
    } = restored;

    let workspace = workspaces.first().expect("workspace should restore");
    let mut public_numbers: Vec<_> = workspace.public_pane_numbers.values().copied().collect();
    public_numbers.sort_unstable();
    assert_eq!(public_numbers, vec![1, 3]);
    assert_eq!(workspace.next_public_pane_number, 4);
    assert_eq!(workspace.tabs[0].number, 5);
    assert_eq!(workspace.next_public_tab_number, 6);
}

#[tokio::test]
async fn cold_restore_with_gapped_public_tab_numbers_drops_unmanaged_agent_name() {
    let cwd = std::env::current_dir().unwrap();
    let pane_snap = |id: &str| {
        (
            id.parse::<u32>().unwrap(),
            super::super::schema::PaneSnapshot {
                cwd: cwd.clone(),
                label: None,
                agent_name: None,
                managed_agent_kind: None,
                agent_session: None,
                launch_argv: None,
            },
        )
    };
    let final_pane = super::super::schema::PaneSnapshot {
        cwd: cwd.clone(),
        label: Some("planner".into()),
        agent_name: Some("planner".into()),
        managed_agent_kind: None,
        agent_session: Some(super::super::schema::PaneAgentSessionSnapshot {
            source: "herdr:codex".into(),
            agent: "codex".into(),
            kind: crate::agents::resume::catalog::AgentSessionRefKind::Id,
            value: "codex-session".into(),
        }),
        launch_argv: None,
    };
    let snapshot = SessionSnapshot {
        version: super::super::schema::SNAPSHOT_VERSION,
        workspaces: vec![WorkspaceSnapshot {
            id: Some("w1".into()),
            custom_name: None,
            identity_cwd: cwd.clone(),
            public_pane_numbers: HashMap::from([(10, 1), (11, 2), (12, 3), (13, 4)]),
            next_public_pane_number: 5,
            public_tab_numbers: vec![1, 3, 4, 5],
            next_public_tab_number: 6,
            tabs: vec![
                TabSnapshot {
                    custom_name: None,
                    layout: LayoutSnapshot::Pane(10),
                    panes: HashMap::from([pane_snap("10")]),
                    zoomed: false,
                    focused: Some(10),
                    root_pane: Some(10),
                },
                TabSnapshot {
                    custom_name: None,
                    layout: LayoutSnapshot::Pane(11),
                    panes: HashMap::from([pane_snap("11")]),
                    zoomed: false,
                    focused: Some(11),
                    root_pane: Some(11),
                },
                TabSnapshot {
                    custom_name: None,
                    layout: LayoutSnapshot::Pane(12),
                    panes: HashMap::from([pane_snap("12")]),
                    zoomed: false,
                    focused: Some(12),
                    root_pane: Some(12),
                },
                TabSnapshot {
                    custom_name: None,
                    layout: LayoutSnapshot::Pane(13),
                    panes: HashMap::from([(13, final_pane)]),
                    zoomed: false,
                    focused: Some(13),
                    root_pane: Some(13),
                },
            ],
            active_tab: 3,
        }],
        active: Some(0),
        selected: 0,
    };
    let (events, _event_rx) = mpsc::channel(4);

    let restored = restore(
        &snapshot,
        None,
        false,
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    );
    let RestoredSession {
        workspaces,
        terminals,
        launches: _runtimes,
        ..
    } = restored;

    let workspace = workspaces.first().expect("workspace should restore");
    assert_eq!(workspace.active_tab, 3);
    assert_eq!(workspace.tabs[3].number, 5);
    let agent_pane = workspace.tabs[3].root_pane;
    let terminal_id = &workspace.tabs[3].panes[&agent_pane].attached_terminal_id;
    assert!(terminals[terminal_id].agent_name.is_none());
    assert_eq!(terminals[terminal_id].managed_agent_kind(), None);
    assert!(workspace
        .pane_details(&terminals)
        .into_iter()
        .all(|detail| detail.pane_id != agent_pane));
}

#[test]
fn legacy_restore_precomputes_missing_public_pane_numbers() {
    let cwd = std::env::current_dir().unwrap();
    let snapshot = WorkspaceSnapshot {
        id: Some("w1".into()),
        custom_name: None,
        identity_cwd: cwd,
        public_pane_numbers: HashMap::new(),
        next_public_pane_number: 0,
        public_tab_numbers: Vec::new(),
        next_public_tab_number: 0,
        tabs: vec![TabSnapshot {
            custom_name: None,
            layout: LayoutSnapshot::Split {
                direction: super::super::schema::DirectionSnapshot::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutSnapshot::Pane(10)),
                second: Box::new(LayoutSnapshot::Pane(20)),
            },
            panes: HashMap::new(),
            zoomed: false,
            focused: Some(10),
            root_pane: Some(10),
        }],
        active_tab: 0,
    };
    let mut next_public_pane_number = 1;

    let public_numbers =
        migrated_public_pane_numbers_by_old_raw(&snapshot, &mut next_public_pane_number);

    assert_eq!(public_numbers, HashMap::from([(10, 1), (20, 2)]));
    assert_eq!(next_public_pane_number, 3);
}

#[tokio::test]
#[cfg(unix)]
async fn native_agent_restore_defers_runtime_launch() {
    let cwd = std::env::current_dir().unwrap();
    let snapshot = SessionSnapshot {
        version: super::super::schema::SNAPSHOT_VERSION,
        workspaces: vec![WorkspaceSnapshot {
            id: Some("workspace".into()),
            custom_name: None,
            identity_cwd: cwd.clone(),
            public_pane_numbers: HashMap::new(),
            next_public_pane_number: 0,
            public_tab_numbers: Vec::new(),
            next_public_tab_number: 0,
            tabs: vec![TabSnapshot {
                custom_name: None,
                layout: LayoutSnapshot::Pane(0),
                panes: HashMap::from([(
                    0,
                    super::super::schema::PaneSnapshot {
                        cwd,
                        label: None,
                        agent_name: None,
                        managed_agent_kind: None,
                        agent_session: Some(super::super::schema::PaneAgentSessionSnapshot {
                            source: "herdr:codex".into(),
                            agent: "codex".into(),
                            kind: crate::agents::resume::catalog::AgentSessionRefKind::Id,
                            value: "codex-session".into(),
                        }),
                        launch_argv: None,
                    },
                )]),
                zoomed: false,
                focused: Some(0),
                root_pane: Some(0),
            }],
            active_tab: 0,
        }],
        active: Some(0),
        selected: 0,
    };
    let (events, _event_rx) = mpsc::channel(4);

    let restored = restore(
        &snapshot,
        None,
        true,
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    );
    let RestoredSession {
        workspaces: _workspaces,
        terminals,
        launches,
        ..
    } = restored;

    let terminal = terminals
        .values()
        .next()
        .expect("native agent restore should create terminal state");
    assert!(
        terminal.pending_agent_resume_plan.is_some(),
        "restored native agent panes should defer resume until client terminal context is known"
    );
    assert!(
        !terminal.respawn_shell_on_exit,
        "deferred agent resume should not use native restore lifecycle before launch"
    );
    assert!(
        launches.is_empty(),
        "native agent restore should not spawn a fallback-size runtime during snapshot restore"
    );
}
#[tokio::test]
async fn restore_seeds_saved_pane_history_into_runtime() {
    let (snapshot, history) = snapshot_with_saved_pane_history();
    crate::server::terminals::restore_launch::tests::assert_history_restore(
        &snapshot,
        Some(&history),
        true,
    );
}

#[tokio::test]
async fn restore_without_history_snapshot_keeps_pane_contents_empty() {
    let (snapshot, _history) = snapshot_with_saved_pane_history();
    crate::server::terminals::restore_launch::tests::assert_history_restore(&snapshot, None, false);
}

#[test]
fn restore_plans_shells_without_requiring_runtime_context() {
    let (snapshot, history) = snapshot_with_saved_pane_history();
    let (events, _events_rx) = mpsc::channel(8);
    // Deliberately outside a Tokio runtime: planning cannot spawn PTY tasks.
    let restored = restore(
        &snapshot,
        Some(&history),
        false,
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    );
    assert_eq!(restored.workspaces.len(), 1);
    assert_eq!(restored.terminals.len(), 1);
    let launch = &restored.launches[0];
    assert_eq!(restored.launches.len(), 1);
    assert_eq!(launch.cwd, snapshot.workspaces[0].identity_cwd);
    assert_eq!(
        launch.initial_history_ansi.as_deref(),
        Some(history.workspaces[0].tabs[0].panes[&0].ansi.as_str())
    );
    let identity = launch.identity.as_ref().unwrap();
    assert_eq!(identity.workspace_id, "workspace");
    assert_eq!(identity.pane_id, "workspace:p1");
    assert_eq!(
        identity.tab_id,
        crate::server::workspaces::public_tab_id_for_number("workspace", 1)
    );
}

fn snapshot_with_saved_pane_history() -> (SessionSnapshot, SessionHistorySnapshot) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let mut panes = HashMap::new();
    panes.insert(
        0,
        super::super::schema::PaneSnapshot {
            cwd: cwd.clone(),
            label: None,
            agent_name: None,
            managed_agent_kind: None,
            agent_session: None,
            launch_argv: None,
        },
    );
    let history = SessionHistorySnapshot {
        version: super::super::schema::SNAPSHOT_VERSION,
        workspaces: vec![WorkspaceHistorySnapshot {
            tabs: vec![super::super::schema::TabHistorySnapshot {
                panes: HashMap::from([(
                    0,
                    super::super::schema::PaneHistorySnapshot {
                        ansi: concat!(
                            "\x1b[31mRESTORED_HISTORY 👨‍👩‍👧\x1b[0m ",
                            "\x1b]8;;https://example.com\x1b\\LINK\x1b]8;;\x1b\\\r\n"
                        )
                        .to_string(),
                        lines: 1,
                    },
                )]),
            }],
        }],
    };
    let snapshot = SessionSnapshot {
        version: super::super::schema::SNAPSHOT_VERSION,
        workspaces: vec![WorkspaceSnapshot {
            id: Some("workspace".into()),
            custom_name: None,
            identity_cwd: cwd,
            public_pane_numbers: HashMap::new(),
            next_public_pane_number: 0,
            public_tab_numbers: Vec::new(),
            next_public_tab_number: 0,
            tabs: vec![TabSnapshot {
                custom_name: None,
                layout: LayoutSnapshot::Pane(0),
                panes,
                zoomed: false,
                focused: Some(0),
                root_pane: Some(0),
            }],
            active_tab: 0,
        }],
        active: Some(0),
        selected: 0,
    };
    (snapshot, history)
}

#[test]
fn failed_restore_of_earlier_tab_preserves_surviving_active_tab() {
    let (mut snapshot, _) = snapshot_with_saved_pane_history();
    let workspace = &mut snapshot.workspaces[0];
    let original_tab = serde_json::to_value(&workspace.tabs[0]).unwrap();
    workspace.tabs = (0..3)
        .map(|index| {
            let mut tab = original_tab.clone();
            tab["custom_name"] = serde_json::json!(format!("tab-{index}"));
            tab["layout"] = serde_json::json!({ "Pane": index });
            tab["focused"] = serde_json::json!(index);
            tab["root_pane"] = serde_json::json!(index);
            let pane = original_tab["panes"]["0"].clone();
            tab["panes"] = serde_json::json!({ index.to_string(): pane });
            serde_json::from_value(tab).unwrap()
        })
        .collect();
    workspace.public_tab_numbers = vec![1, 2, 3];
    workspace.active_tab = 1;
    let (events, _events_rx) = mpsc::channel(8);
    let mut restored = restore(
        &snapshot,
        None,
        false,
        events,
        Arc::new(Notify::new()),
        Arc::new(RenderSignal::new()),
    );
    let workspace = &restored.workspaces[0];
    assert_eq!(workspace.tabs[workspace.active_tab].number, 2);
    let failed = HashSet::from([restored.launches[0].terminal_id.clone()]);

    restored.discard_failed_launches(&failed);

    let workspace = &restored.workspaces[0];
    assert_eq!(workspace.tabs.len(), 2);
    assert_eq!(workspace.tabs[workspace.active_tab].number, 2);
}
