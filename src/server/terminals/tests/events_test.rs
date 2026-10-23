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
