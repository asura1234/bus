use super::*;

use crate::detect::{Agent, AgentState};

use crate::workspace::Workspace;

use ratatui::layout::Direction;

fn app_with_workspaces(names: &[&str]) -> AppState {
    let mut state = AppState::test_new();
    state.toast_config.delay_seconds = 0;
    for name in names {
        let ws = Workspace::test_new(name);
        state.workspaces.push(ws);
    }
    state.ensure_test_terminals();
    if !state.workspaces.is_empty() {
        state.active = Some(0);
        state.mode = Mode::Terminal;
    }
    state
}

#[test]
fn notification_context_formats_resolved_workspace_label() {
    let state = app_with_workspaces(&["stale"]);
    let root = state.workspaces[0].tabs[0].root_pane;

    assert_eq!(
        notification_context(&state.workspaces[0], "__herdr_projects__", 0, root),
        "__herdr_projects__ · 1"
    );
}

fn selected_word(row: &str, col: u16) -> Option<String> {
    let (start, end) = word_bounds_at_column(row, col)?;
    Some(text_in_cell_range(row, start, end))
}

fn selected_url<'a>(row: &'a str, click: &str) -> Option<&'a str> {
    url_at_column(row, col_of(row, click))
}

fn text_in_cell_range(row: &str, start_col: u16, end_col: u16) -> String {
    text_cells(row)
        .into_iter()
        .filter(|cell| cell.start_col >= start_col && cell.end_col <= end_col)
        .map(|cell| cell.ch)
        .collect()
}

fn col_of(row: &str, needle: &str) -> u16 {
    let byte_idx = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not found in {row:?}"));
    let prefix = &row[..byte_idx];
    prefix
        .chars()
        .map(|ch| u16::from(crate::ghostty::unicode_codepoint_width(ch as u32)))
        .sum()
}

fn assert_selects(row: &str, click: &str, expected: &str) {
    assert_eq!(
        selected_word(row, col_of(row, click)).as_deref(),
        Some(expected),
        "row={row:?}, click={click:?}"
    );
}

fn assert_selects_nothing(row: &str, click: &str) {
    assert_eq!(
        selected_word(row, col_of(row, click)),
        None,
        "row={row:?}, click={click:?}"
    );
}

#[test]
fn double_click_word_bounds_cover_terminal_text() {
    let cases = [
        (
            "see https://example.com/a-b_c?q=x@y.",
            "example.com",
            "https://example.com/a-b_c?q=x@y",
        ),
        (
            "open \"https://example.com/a,b;c?q=x\";",
            "example.com",
            "https://example.com/a,b;c?q=x",
        ),
        (
            "see https://en.wikipedia.org/wiki/Foo_(bar_(baz)),",
            "wikipedia",
            "https://en.wikipedia.org/wiki/Foo_(bar_(baz))",
        ),
        (
            "see https://example.com/a(b[c{d}e]f),",
            "example.com",
            "https://example.com/a(b[c{d}e]f)",
        ),
        (
            "see (https://example.com/a(b(c)d)))",
            "example.com",
            "https://example.com/a(b(c)d)",
        ),
        (
            "open /tmp/foo-bar/baz_qux/",
            "foo-bar",
            "/tmp/foo-bar/baz_qux/",
        ),
        (
            "open ./src/app/actions.rs:795",
            "actions",
            "./src/app/actions.rs:795",
        ),
        (
            "open ../herdr-worktrees/issue-1",
            "herdr",
            "../herdr-worktrees/issue-1",
        ),
        (
            "edit src/app/actions.rs,then",
            "actions",
            "src/app/actions.rs",
        ),
        (
            "cat \"/tmp/build output/log.txt\"",
            "output",
            "/tmp/build output/log.txt",
        ),
        (
            "cat '/Users/me/Library/Application Support/app/config.json'",
            "Support",
            "/Users/me/Library/Application Support/app/config.json",
        ),
        ("echo 你好-world done", "好", "你好-world"),
        ("先跑 cargo test", "cargo", "cargo"),
        (
            "export PATH=$HOME/.cargo/bin:$PATH",
            "$HOME",
            "PATH=$HOME/.cargo/bin:$PATH",
        ),
        (
            "git checkout feature/foo-bar_baz",
            "foo",
            "feature/foo-bar_baz",
        ),
        ("refs #123 and @owner/name", "#123", "#123"),
        ("refs #123 and @owner/name", "owner", "@owner/name"),
        ("cargo test --package=herdr", "--package", "--package=herdr"),
        (
            "cargo test app::actions::tests",
            "app::",
            "app::actions::tests",
        ),
        (
            "image ghcr.io/org/app:latest",
            "ghcr",
            "ghcr.io/org/app:latest",
        ),
        ("ERROR [worker-1] request_id=abc-123", "worker", "worker-1"),
        (
            "tmux|newhoo|fixhoo|newmoo|notification|window_bell|herdr",
            "newhoo",
            "newhoo",
        ),
        (
            "render_status_line(app, area)",
            "render",
            "render_status_line",
        ),
        ("render_status_line(app, area)", "app", "app"),
        ("render_status_line(app, area)", "area", "area"),
        ("if !enabled {", "enabled", "enabled"),
        ("println!(\"hi\")", "println", "println"),
        ("( master)$", "master", "master"),
        ("regex foo$", "foo", "foo$"),
    ];

    for (row, click, expected) in cases {
        assert_selects(row, click, expected);
    }

    let row = "echo 你好-world done";
    assert_eq!(
        selected_word(row, col_of(row, "好") + 1).as_deref(),
        Some("你好-world")
    );
}

#[test]
fn double_click_word_bounds_ignore_delimiters() {
    for (row, click) in [
        (
            "tmux|newhoo|fixhoo|newmoo|notification|window_bell|herdr",
            "|",
        ),
        ("alpha,beta;gamma", ","),
        ("alpha,beta;gamma", ";"),
        ("render_status_line(app, area)", "("),
        ("render_status_line(app, area)", ")"),
        ("if !enabled {", "!"),
        ("if !enabled {", "{"),
        ("(done).", "("),
        ("(done).", "."),
    ] {
        assert_selects_nothing(row, click);
    }
}

#[test]
fn url_at_column_returns_safe_visible_url_only() {
    assert_eq!(
        selected_url("see https://example.com/a(b)c.", "example"),
        Some("https://example.com/a(b)c")
    );
    assert_eq!(
        selected_url("[docs](https://example.com/docs),", "example"),
        Some("https://example.com/docs")
    );
    assert_eq!(
        selected_url("[docs](https://example.com/docs)", "docs"),
        None
    );
    assert_eq!(selected_url("open file:///tmp/report", "file"), None);
}

#[test]
fn switch_workspace_updates_active_and_selected() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    state.switch_workspace(2);
    assert_eq!(state.active, Some(2));
    assert_eq!(state.selected, 2);
}

#[test]
fn switch_workspace_marks_panes_seen() {
    let mut state = app_with_workspaces(&["a", "b"]);
    // Mark a pane in workspace 1 as unseen
    let id = *state.workspaces[1].panes.keys().next().unwrap();
    state.workspaces[1].panes.get_mut(&id).unwrap().seen = false;

    state.switch_workspace(1);
    assert!(state.workspaces[1].panes.get(&id).unwrap().seen);
}

#[test]
fn switch_workspace_out_of_bounds_is_noop() {
    let mut state = app_with_workspaces(&["a"]);
    state.switch_workspace(5);
    assert_eq!(state.active, Some(0));
}

#[test]
fn move_workspace_reorders_without_changing_logical_selection() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    let active_id = state.workspaces[1].id.clone();
    let selected_id = state.workspaces[2].id.clone();
    state.active = Some(1);
    state.selected = 2;

    state.move_workspace(1, 0);

    let names: Vec<_> = state
        .workspaces
        .iter()
        .map(|ws| ws.display_name())
        .collect();
    assert_eq!(names, vec!["b", "a", "c"]);
    assert_eq!(state.active, Some(0));
    assert_eq!(state.selected, 2);
    assert_eq!(state.workspaces[state.active.unwrap()].id, active_id);
    assert_eq!(state.workspaces[state.selected].id, selected_id);
}

#[test]
fn move_workspace_accepts_insert_at_end() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);

    state.move_workspace(0, state.workspaces.len());

    let names: Vec<_> = state
        .workspaces
        .iter()
        .map(|ws| ws.display_name())
        .collect();
    assert_eq!(names, vec!["b", "c", "a"]);
}

#[test]
fn close_workspace_adjusts_indices() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    state.selected = 1;
    state.active = Some(1);

    state.close_selected_workspace();

    assert_eq!(state.workspaces.len(), 2);
    assert_eq!(state.selected, 1);
    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].custom_name.as_deref(), Some("c"));
}

#[test]
fn close_last_workspace_clears_active() {
    let mut state = app_with_workspaces(&["only"]);
    state.selected = 0;
    state.close_selected_workspace();

    assert!(state.workspaces.is_empty());
    assert_eq!(state.active, None);
    assert_eq!(state.selected, 0);
}

#[test]
fn close_workspace_at_end_adjusts_selected() {
    let mut state = app_with_workspaces(&["a", "b"]);
    state.selected = 1;
    state.active = Some(1);

    state.close_selected_workspace();

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.selected, 0);
    assert_eq!(state.active, Some(0));
}

#[test]
fn close_non_focused_workspace_keeps_focus() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    state.selected = 1;
    state.active = Some(0);

    state.close_selected_workspace();

    assert_eq!(state.workspaces.len(), 2);
    assert_eq!(state.workspaces[0].display_name(), "a");
    assert_eq!(state.workspaces[1].display_name(), "c");
    assert_eq!(state.selected, 0);
    assert_eq!(state.active, Some(0));
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_last_pane_removes_workspace() {
    let mut state = app_with_workspaces(&["a", "b"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_pane_died(pane_id);

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].custom_name.as_deref(), Some("b"));
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_last_workspace_enters_navigate() {
    let mut state = app_with_workspaces(&["only"]);
    state.mode = Mode::Terminal;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_pane_died(pane_id);

    assert!(state.workspaces.is_empty());
    assert_eq!(state.mode, Mode::Navigate);
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_multi_pane_keeps_workspace() {
    let mut state = app_with_workspaces(&["test"]);
    let second_id = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();

    state.handle_pane_died(second_id);

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].panes.len(), 1);
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_unknown_pane_is_noop() {
    let mut state = app_with_workspaces(&["test"]);
    let fake_id = PaneId::from_raw(9999);

    state.handle_pane_died(fake_id);

    assert_eq!(state.workspaces.len(), 1);
    state.assert_invariants_for_test();
}

#[test]
fn state_changed_updates_pane() {
    let mut state = app_with_workspaces(&["test"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let terminal_id = state.workspaces[0]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Working);
    assert_eq!(terminal.detected_agent, Some(Agent::Pi));
}

#[test]
fn state_changed_idle_in_background_marks_unseen() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.active = Some(0);
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    // First set it to Working
    let bg_terminal_id = state.workspaces[1]
        .panes
        .get(&bg_pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    state.terminals.get_mut(&bg_terminal_id).unwrap().state = AgentState::Working;

    // Now transition to Idle while in background
    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let pane = state.workspaces[1].panes.get(&bg_pane_id).unwrap();
    assert!(!pane.seen);
    assert!(matches!(
        state.toast.as_ref().map(|toast| toast.kind),
        Some(ToastKind::Finished)
    ));
}

#[test]
fn active_tab_completion_marks_pane_seen() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.outer_terminal_focus = Some(true);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let terminal_id = state.workspaces[0]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    state.terminals.get_mut(&terminal_id).unwrap().state = AgentState::Working;
    state.workspaces[0].panes.get_mut(&pane_id).unwrap().seen = false;

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Idle);
    let pane = state.workspaces[0].panes.get(&pane_id).unwrap();
    assert!(pane.seen);
}

#[test]
fn initial_idle_in_background_stays_seen() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let pane = state.workspaces[1].panes.get(&bg_pane_id).unwrap();
    assert!(pane.seen);
}

#[test]
fn idle_after_known_unknown_agent_in_background_marks_done() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.active = Some(0);
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Unknown,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let pane = state.workspaces[1].panes.get(&bg_pane_id).unwrap();
    assert!(!pane.seen);
}

#[test]
fn first_idle_after_process_detection_is_not_completion() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.active = Some(0);
    let pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::AgentProcessDetected {
        pane_id,
        agent: Agent::Pi,
        observed_at: Instant::now(),
    });
    let direct_idle = state
        .handle_app_event(AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Pi),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: false,
            observed_at: Instant::now(),
        })
        .pop()
        .expect("direct idle state update");
    assert!(direct_idle.suppress_completion);

    state.handle_app_event(AppEvent::AgentProcessDetected {
        pane_id,
        agent: Agent::Pi,
        observed_at: Instant::now(),
    });
    for agent_state in [AgentState::Working, AgentState::Blocked] {
        state.handle_app_event(AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Pi),
            state: agent_state,
            visible_blocker: agent_state == AgentState::Blocked,
            process_exited: false,
            observed_at: Instant::now(),
        });
    }
    let update = state
        .handle_app_event(AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Pi),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: false,
            observed_at: Instant::now(),
        })
        .pop()
        .expect("idle state update");

    assert!(update.suppress_completion);
    assert!(state.workspaces[1].panes[&pane_id].seen);
    assert!(!matches!(
        state.toast.as_ref().map(|toast| toast.kind),
        Some(ToastKind::Finished)
    ));

    state.handle_app_event(AppEvent::AgentProcessDetected {
        pane_id,
        agent: Agent::Codex,
        observed_at: Instant::now(),
    });
    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Codex),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        observed_at: Instant::now(),
    });
    let exit_update = state
        .handle_app_event(AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Codex),
            state: AgentState::Idle,
            visible_blocker: false,
            process_exited: true,
            observed_at: Instant::now(),
        })
        .pop()
        .expect("process exit update");
    assert!(!exit_update.suppress_completion);
}

#[test]
fn background_waiting_sets_attention_toast() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "background · 2");
}

#[test]
fn delayed_background_waiting_schedules_before_toast() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.toast_config.delay_seconds = 1;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.toast.is_none());
    assert!(state.pending_agent_notifications.contains_key(&bg_pane_id));

    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    let deliveries = state.drain_due_agent_notifications(deadline);
    assert_eq!(deliveries.len(), 1);

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "background · 2");
    assert!(state.pending_agent_notifications.is_empty());
}

#[test]
fn delayed_background_waiting_cancels_when_agent_resumes_working() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.toast_config.delay_seconds = 1;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    let deadline = state.next_pending_agent_notification_deadline().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Working,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.pending_agent_notifications.is_empty());
    assert!(state.drain_due_agent_notifications(deadline).is_empty());
    assert!(state.toast.is_none());
}

#[test]
fn delayed_background_waiting_is_suppressed_if_pane_becomes_active() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.toast_config.delay_seconds = 1;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    state.active = Some(1);

    assert!(state.drain_due_agent_notifications(deadline).is_empty());
    assert!(state.toast.is_none());
}

#[test]
fn delayed_active_tab_unfocused_keeps_client_notification_available() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.outer_terminal_focus = Some(false);
    state.toast_config.delivery = crate::config::ToastDelivery::System;
    state.toast_config.delay_seconds = 1;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    let deliveries = state.drain_due_agent_notifications(deadline);

    assert_eq!(deliveries.len(), 1);
    assert!(deliveries[0].toast.is_none());
    assert!(deliveries[0].client_notification.is_some());
    assert!(state.toast.is_none());
}

#[test]
fn delayed_background_waiting_is_cleared_when_pane_dies() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.toast_config.delay_seconds = 1;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    state.handle_app_event(AppEvent::PaneDied {
        pane_id: bg_pane_id,
        exit_reason: crate::platform::ChildExitReason::Exited,
    });

    assert!(state.pending_agent_notifications.is_empty());
    assert!(state.drain_due_agent_notifications(deadline).is_empty());
    assert!(state.toast.is_none());
}

#[test]
fn terminal_cwd_report_updates_terminal_cwd_and_marks_session_dirty() {
    let mut state = app_with_workspaces(&["active"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let terminal_id = state.workspaces[0]
        .pane_state(pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    let cwd = std::env::temp_dir().join(format!("herdr-cwd-report-test-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    state.session_dirty = false;

    let updates = state.handle_app_event(AppEvent::TerminalCwdReported {
        pane_id,
        cwd: cwd.clone(),
    });

    assert!(updates.is_empty());
    assert_eq!(state.terminals.get(&terminal_id).unwrap().cwd, cwd);
    assert!(state.session_dirty);
    let _ = std::fs::remove_dir_all(cwd);
}

#[test]
fn background_idle_sets_finished_toast() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();
    let bg_terminal_id = state.workspaces[1]
        .panes
        .get(&bg_pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    state.terminals.get_mut(&bg_terminal_id).unwrap().state = AgentState::Working;

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Droid),
        state: AgentState::Idle,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::Finished);
    assert_eq!(toast.title, "droid finished");
    assert_eq!(toast.context, "background · 2");
    let target = toast.target.as_ref().expect("toast target");
    assert_eq!(&target.workspace_id, &state.workspaces[1].id);
    assert_eq!(target.pane_id, bg_pane_id);
}

#[test]
fn background_toast_includes_tab_name_when_workspace_has_multiple_tabs() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.workspaces[1].tabs[0].set_custom_name("main".into());
    let second_tab = state.workspaces[1].test_add_tab(Some("logs"));
    state.ensure_test_terminals();
    let bg_pane_id = state.workspaces[1].tabs[second_tab].root_pane;

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "background · 2 · logs");
}

#[test]
fn background_tab_in_active_workspace_still_sets_toast() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.workspaces[0].tabs[0].set_custom_name("main".into());
    let second_tab = state.workspaces[0].test_add_tab(Some("logs"));
    state.ensure_test_terminals();
    let bg_pane_id = state.workspaces[0].tabs[second_tab].root_pane;

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "active · 1 · logs");
}

#[test]
fn active_workspace_active_tab_does_not_set_toast() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.toast.is_none());
}

#[test]
fn active_workspace_active_tab_keeps_herdr_toast_suppressed_when_outer_terminal_is_unfocused() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.outer_terminal_focus = Some(false);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.toast.is_none());
}

#[test]
fn active_tab_suppression_preserves_unknown_focus_behavior() {
    assert!(active_tab_suppresses_notifications(true, None));
    assert!(active_tab_suppresses_notifications(true, Some(true)));
    assert!(!active_tab_suppresses_notifications(true, Some(false)));
    assert!(!active_tab_suppresses_notifications(false, None));
}

#[test]
fn toggle_zoom_works() {
    let mut state = app_with_workspaces(&["test"]);
    state.workspaces[0].test_split(Direction::Horizontal);

    assert!(!state.workspaces[0].zoomed);
    state.toggle_zoom();
    assert!(state.workspaces[0].zoomed);
    state.toggle_zoom();
    assert!(!state.workspaces[0].zoomed);
}

#[test]
fn toggle_zoom_single_pane_noop() {
    let mut state = app_with_workspaces(&["test"]);
    state.toggle_zoom();
    assert!(!state.workspaces[0].zoomed);
}

#[test]
fn navigate_pane_changes_focus_while_zoomed() {
    let mut state = app_with_workspaces(&["test"]);
    let root = state.workspaces[0].tabs[0].root_pane;
    let right = state.workspaces[0].test_split(Direction::Horizontal);
    state.workspaces[0].layout.focus_pane(root);
    state.workspaces[0].zoomed = true;
    crate::ui::compute_view_with_runtime_registry(
        &mut state,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        ratatui::layout::Rect::new(0, 0, 100, 20),
    );

    assert_eq!(state.view.pane_infos.len(), 1);
    assert_eq!(state.view.pane_infos[0].id, root);

    state.navigate_pane(NavDirection::Right);
    crate::ui::compute_view_with_runtime_registry(
        &mut state,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        ratatui::layout::Rect::new(0, 0, 100, 20),
    );

    assert!(state.workspaces[0].zoomed);
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(right));
    assert_eq!(state.view.pane_infos.len(), 1);
    assert_eq!(state.view.pane_infos[0].id, right);
    assert!(state.view.pane_infos[0].inner_rect.x > state.view.pane_infos[0].rect.x);
}

#[test]
fn close_pane_removes_from_workspace() {
    let mut state = app_with_workspaces(&["test"]);
    state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();
    assert_eq!(state.workspaces[0].panes.len(), 2);
    state.close_pane();
    assert_eq!(state.workspaces[0].panes.len(), 1);
    state.assert_invariants_for_test();
}

#[test]
fn pane_process_exit_publish_marks_agent_idle_before_pane_removal() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.active = Some(1);
    state.ensure_test_terminals();
    let pane_id = state.workspaces[0].tabs[0].root_pane;
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();
    state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(Agent::Pi), AgentState::Working);
    assert_eq!(
        state.terminals.get(&terminal_id).unwrap().state,
        AgentState::Working
    );

    let update = state
        .publish_pane_process_exit_if_agent(pane_id, false)
        .expect("process exit update");

    assert!(!state.pane_is_in_active_tab(update.ws_idx, pane_id));
    assert_eq!(update.previous_state, AgentState::Working);
    assert_eq!(update.state, AgentState::Idle);
    assert_eq!(update.agent_label.as_deref(), Some("pi"));
    assert_eq!(update.known_agent, Some(Agent::Pi));
    assert!(update.agent_released);
    assert_eq!(
        update.agent_release_status,
        Some(crate::api::schema::AgentStatus::Done)
    );
    assert!(matches!(
        state.toast.as_ref().map(|toast| toast.kind),
        Some(ToastKind::Finished)
    ));
}

#[test]
fn close_pane_removes_unattached_terminal_state() {
    let mut state = app_with_workspaces(&["test"]);
    let pane_id = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();

    state.close_pane();

    assert!(!state.terminals.contains_key(&terminal_id));
    state.assert_invariants_for_test();
}

#[test]
fn close_workspace_removes_unattached_terminal_states() {
    let mut state = app_with_workspaces(&["one", "two"]);
    let pane_id = state.workspaces[0].tabs[0].root_pane;
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();
    state.close_selected_workspace();

    assert!(!state.terminals.contains_key(&terminal_id));
    state.assert_invariants_for_test();
}

#[test]
fn close_pane_last_pane_closes_active_workspace_not_selected_workspace() {
    let mut state = app_with_workspaces(&["selected", "active"]);
    let active_terminal_id = state
        .terminal_id_for_pane(1, state.workspaces[1].tabs[0].root_pane)
        .unwrap();
    state.active = Some(1);
    state.selected = 0;

    state.close_pane();

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].display_name(), "selected");
    assert!(!state.terminals.contains_key(&active_terminal_id));
    state.assert_invariants_for_test();
}
