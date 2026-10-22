use super::*;
use crate::utils::text::hit_testing::test_support::text_in_cell_range;

use crate::agents::{AgentKind, AgentState};

use crate::server::workspaces::Workspace;

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

fn selected_word(row: &str, col: u16) -> Option<String> {
    let (start, end) = word_bounds_at_column(row, col, terminal_char_width)?;
    Some(text_in_cell_range(row, start, end, terminal_char_width))
}

fn selected_url<'a>(row: &'a str, click: &str) -> Option<&'a str> {
    url_at_column(row, col_of(row, click), terminal_char_width)
}

fn col_of(row: &str, needle: &str) -> u16 {
    let byte_idx = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not found in {row:?}"));
    let prefix = &row[..byte_idx];
    prefix
        .chars()
        .map(|ch| {
            u16::from(crate::utils::text::width::unicode_codepoint_width(
                ch as u32,
            ))
        })
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

include!("../../notifications/policy_test.rs");

include!("../../workspaces/tests/navigation_test.rs");

include!("../../workspaces/tests/close_test.rs");

include!("../../../utils/text/hit_test_test.rs");

mod codex_self_update_listing {
    use crate::agents::{AgentKind, AgentState};
    use crate::protocol::api::schema;
    use crate::server::app::App;
    use crate::server::workspaces::Workspace;
    use crate::terminal::events::TerminalEvent;

    const CHOOSER: &[u8] =
        include_bytes!("../../../../tests/fixtures/codex-update/chooser-0.162.ansi");

    fn state_changed(pane_id: crate::utils::ids::PaneId, state: AgentState) -> TerminalEvent {
        TerminalEvent::StateChanged {
            pane_id,
            agent: Some(AgentKind::Codex),
            state,
            visible_blocker: state == AgentState::Blocked,
            process_exited: false,
            exited_process: None,
            observed_at: std::time::Instant::now(),
        }
    }

    #[tokio::test]
    async fn never_selects_update_now_even_for_native_agents() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::utils::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("update"), Workspace::test_new("other")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(120, 40, 16);
        app.terminal_runtimes.insert(terminal_id.clone(), runtime);
        let pane_id = app.public_pane_id(0, pane).unwrap();
        let started = app.handle_api_request(schema::Request {
            id: "start".into(),
            method: schema::Method::AgentStart(schema::AgentStartParams {
                name: "native-codex".into(),
                kind: "codex".into(),
                pane_id,
                args: vec!["-c".into(), "hooks=bus".into()],
                executable: None,
                timeout_ms: Some(4_000),
            }),
        });
        assert!(started.contains("agent_started"), "{started}");
        writes.try_recv().unwrap();

        // Codex draws its chooser; the screen event reaches the app.
        app.terminal_runtimes
            .get(&terminal_id)
            .unwrap()
            .test_process_pty_bytes(CHOOSER);
        app.handle_internal_event(state_changed(pane, AgentState::Blocked));
        assert!(
            writes.try_recv().is_err(),
            "Bus must never choose Update now"
        );
        let info = app.agent_info(0, pane).unwrap();
        assert_eq!(info.agent_status, schema::AgentStatus::Blocked);
        assert!(info.dialog_id.is_some());
    }
}

#[test]
fn session_capture_keeps_restored_claude_name_while_its_resume_has_not_started() {
    let mut state = app_with_workspaces(&["ws"]);
    let terminal_id = state.workspaces[0].tabs[0]
        .panes
        .values()
        .next()
        .unwrap()
        .attached_terminal_id
        .clone();
    let terminal = state.terminals.get_mut(&terminal_id).unwrap();
    terminal.restore_managed_agent("ally".into(), AgentKind::Claude);
    terminal.pending_agent_resume_plan = Some(crate::agents::resume::catalog::AgentResumePlan {
        agent: "claude".into(),
        argv: vec!["claude".into(), "--resume".into(), "saved".into()],
        dedupe_key: "saved".into(),
    });
    let snapshot = crate::server::persistence::capture(
        &state.workspaces,
        &state.terminals,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        state.active,
        0,
    );
    let pane = snapshot.workspaces[0].tabs[0]
        .panes
        .values()
        .next()
        .unwrap();
    assert_eq!(pane.agent_name.as_deref(), Some("ally"));
    assert_eq!(pane.managed_agent_kind.as_deref(), Some("claude"));
}

mod grok_captured_composer_prompt {
    use crate::agents::{AgentKind, AgentState};
    use crate::protocol::api::schema::{AgentPromptIfIdleParams, Method, Request};
    use crate::server::app::App;
    use crate::server::workspaces::Workspace;
    use std::time::Duration;

    /// The idle Grok Build 1.0.5 fullscreen frame rendered from the offline
    /// capture: the model label is drawn inside the composer's bottom border.
    fn captured_idle_frame() -> String {
        // Byte-identical offline capture; replay through its first complete
        // normal-mode shortcut footer, retaining the native labelled border.
        let capture =
            include_str!("../../../../tests/fixtures/grok-composer/native-1.0.5-offline.ansi");
        let footer = capture.find(":shortcuts").unwrap();
        let end = footer + capture[footer..].find("\x1b[?2026l").unwrap() + "\x1b[?2026l".len();
        capture[..end].to_owned()
    }

    fn captured_layout(label: &str) -> String {
        let width = 116;
        let top = format!("  ╭{}╮", "─".repeat(width));
        let row = format!("  │ ❯{}│", " ".repeat(width - 2));
        let bottom = format!(
            "  ╰{}{label}",
            "─".repeat(width + 1 - label.chars().count())
        );
        format!(
            "\x1b[2J\x1b[H\r\n  /private/tmp/workspace\r\n\r\n     ◆ session_start  [hooks: 1]\r\n\r\n{top}\r\n{row}\r\n{bottom}\r\n\r\n  Shift+Tab:mode  │  Ctrl+x:shortcuts"
        )
    }

    fn synthetic_idle_frame() -> String {
        include_str!("../../../../tests/fixtures/grok-composer/empty.txt").replace('\n', "\r\n")
    }

    async fn first_write_for(frame: String) -> Option<bytes::Bytes> {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::utils::config::Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("grok")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let now = std::time::Instant::now();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.begin_managed_agent(
            "grok-worker".into(),
            AgentKind::Grok,
            now,
            Duration::ZERO,
            Duration::from_secs(10),
        );
        terminal.set_detected_state(Some(AgentKind::Grok), AgentState::Idle);
        terminal.reconcile_managed_agent_at(now + Duration::from_secs(1), false);
        terminal.set_agent_session_ref_for_session_start(
            "bus".into(),
            "grok".into(),
            crate::agents::resume::catalog::AgentSessionRef::id("grok-session"),
            Some(1),
            None,
            &[],
        );
        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(120, 40, 32);
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        runtime.test_process_pty_bytes(frame.as_bytes());
        app.state.insert_test_runtime(pane, runtime);
        let info = app.agent_info(0, pane).unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let _ = app.handle_deferred_agent_api_request(
            Request {
                id: "grok-prompt".into(),
                method: Method::AgentPromptIfIdle(AgentPromptIfIdleParams {
                    target: info.pane_id.clone(),
                    text: "hello grok".into(),
                    expected_terminal_id: info.terminal_id,
                    expected_pane_id: info.pane_id,
                    expected_agent: "grok".into(),
                    expected_session_id: "grok-session".into(),
                    steer: false,
                }),
            },
            tx,
        );
        tokio::time::timeout(Duration::from_secs(2), writes.recv())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test]
    async fn bus_prompt_is_pasted_into_the_synthetic_empty_grok_composer() {
        let write = first_write_for(synthetic_idle_frame()).await;
        assert!(
            write.is_some_and(|w| w.as_ref().starts_with(b"\x1b[200~hello grok")),
            "synthetic composer should receive the paste"
        );
    }

    #[tokio::test]
    async fn bus_prompt_is_pasted_into_the_captured_grok_layout_when_its_border_has_no_label() {
        let write = first_write_for(captured_layout("──╯")).await;
        assert!(
            write.is_some_and(|w| w.as_ref().starts_with(b"\x1b[200~hello grok")),
            "the captured layout without a border label should receive the paste"
        );
    }

    #[tokio::test]
    async fn bus_prompt_is_pasted_into_the_captured_grok_1_0_5_composer_with_model_label_border() {
        let write = first_write_for(captured_idle_frame()).await;
        assert!(
            write.is_some_and(|w| w.as_ref().starts_with(b"\x1b[200~hello grok")),
            "the captured empty Grok 1.0.5 composer must receive the Bus prompt"
        );
    }
}
