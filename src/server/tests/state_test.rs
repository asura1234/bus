use super::*;

#[test]
fn pane_size_estimate_uses_headless_size_before_first_view() {
    let mut state = AppState::test_new();
    state.headless_size = (132, 41);

    assert_eq!(state.estimate_pane_size(), (41, 132));
}

#[test]
fn adversarial_identity_state_satisfies_app_invariants_after_mutation() {
    let mut state = AppState::test_with_adversarial_identity_state();
    state.assert_invariants_for_test();

    let ws = &mut state.workspaces[0];
    let active_public = ws.tabs[ws.active_tab].number;
    assert_ne!(ws.active_tab + 1, active_public);
    let new_pane = ws.test_split(ratatui::layout::Direction::Horizontal);
    assert!(ws.public_pane_number(new_pane).is_some());
    state.ensure_test_terminals();

    state.assert_invariants_for_test();
}

include!("../../utils/theme/theme_test.rs");
