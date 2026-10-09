use super::*;
use crate::server::workspaces::Workspace;

#[test]
fn hidden_render_attempt_keeps_presentation_cadence_available() {
    let (mut app, _) = test_app_with_pane();
    let initial_presentation = Instant::now();
    app.record_render_attempt(initial_presentation, true);

    let hidden_attempt = initial_presentation + MIN_RENDER_INTERVAL;
    app.record_render_attempt(hidden_attempt, false);
    let foreground_echo = hidden_attempt + Duration::from_millis(1);

    assert!(!app.can_render_now(foreground_echo));
    assert!(app.can_present_now(foreground_echo));
}

#[test]
fn interrupted_detached_process_wait_keeps_child_for_retry() {
    let interrupted = std::io::Error::new(std::io::ErrorKind::Interrupted, "test interrupt");

    assert!(retain_detached_process_after_wait(42, Err(interrupted)));
}

fn test_app_with_pane() -> (App, crate::utils::ids::PaneId) {
    let mut app = App::new(
        &crate::utils::config::Config::default(),
        crate::server::app::AppPolicy::TEST,
        None,
        tokio::sync::mpsc::unbounded_channel().1,
        crate::server::api::EventHub::default(),
    );
    let ws = Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    app.state.workspaces.push(ws);
    app.state.active = Some(0);
    app.state
        .view
        .pane_infos
        .push(crate::server::workspaces::layout::PaneInfo {
            id: pane_id,
            rect: ratatui::layout::Rect::new(0, 0, 80, 24),
            inner_rect: ratatui::layout::Rect::new(0, 0, 80, 24),
            scrollbar_rect: None,
            borders: ratatui::widgets::Borders::NONE,
            is_focused: true,
        });
    (app, pane_id)
}
