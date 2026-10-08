use super::*;
use crate::utils::text::hit_testing::test_support::text_in_cell_range;

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

fn selected_word(row: &str, col: u16) -> Option<String> {
    let (start, end) = word_bounds_at_column(row, col)?;
    Some(text_in_cell_range(row, start, end))
}

fn selected_url<'a>(row: &'a str, click: &str) -> Option<&'a str> {
    url_at_column(row, col_of(row, click))
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

include!("../../notifications/policy_test.rs");

include!("../../workspaces/tests/navigation_test.rs");

include!("../../workspaces/tests/close_test.rs");

include!("../../../utils/text/hit_test_test.rs");
