#[tokio::test]
async fn pane_scrollbar_gutter_is_reserved_before_scrollback_exists() {
    let mut app = AppState::test_new();
    let mut workspace = Workspace::test_new("test");
    let root_pane = workspace.tabs[0].root_pane;
    workspace.tabs[0].runtimes.insert(
        root_pane,
        TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
    );
    app.workspaces = vec![workspace];
    app.active = Some(0);

    let area = Rect::new(10, 3, 40, 8);
    let terminal_runtimes = TerminalRuntimeRegistry::new();
    let infos = compute_pane_infos(
        &app,
        &terminal_runtimes,
        area,
        false,
        crate::kitty_graphics::HostCellSize::default(),
    );
    let info = &infos[0];

    assert_eq!(info.rect, area);
    assert_eq!(info.scrollbar_rect, None);
    assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));
}

#[tokio::test]
async fn alternate_screen_reclaims_scrollbar_gutter_and_restores_it_on_exit() {
    let mut app = AppState::test_new();
    let mut workspace = Workspace::test_new("test");
    let root_pane = workspace.tabs[0].root_pane;
    workspace.tabs[0].runtimes.insert(
        root_pane,
        TerminalRuntime::test_with_scrollback_bytes(
            40,
            8,
            1024,
            b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
        ),
    );
    app.workspaces = vec![workspace];
    app.active = Some(0);

    let area = Rect::new(10, 3, 40, 8);
    let terminal_runtimes = TerminalRuntimeRegistry::new();
    let assert_geometry = |expected_width, has_scrollbar| {
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            true,
            crate::kitty_graphics::HostCellSize::default(),
        );
        assert_eq!(
            infos[0].inner_rect,
            Rect::new(area.x, area.y, expected_width, area.height)
        );
        assert_eq!(infos[0].scrollbar_rect.is_some(), has_scrollbar);
        assert_eq!(
            app.workspaces[0].tabs[0].runtimes[&root_pane].current_size(),
            (area.height, expected_width)
        );
    };

    assert_geometry(39, true);
    app.workspaces[0].tabs[0].runtimes[&root_pane].test_process_pty_bytes(b"\x1b[?1049h");
    assert_geometry(40, false);
    app.workspaces[0].tabs[0].runtimes[&root_pane].test_process_pty_bytes(b"\x1b[?1049l");
    assert_geometry(39, true);
}

#[tokio::test]
async fn zoomed_pane_scrollbar_gutter_is_reserved_before_scrollback_exists() {
    let mut app = AppState::test_new();
    let mut workspace = Workspace::test_new("test");
    workspace.zoomed = true;
    let root_pane = workspace.tabs[0].root_pane;
    workspace.tabs[0].runtimes.insert(
        root_pane,
        TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
    );
    app.workspaces = vec![workspace];
    app.active = Some(0);

    let area = Rect::new(10, 3, 40, 8);
    let terminal_runtimes = TerminalRuntimeRegistry::new();
    let infos = compute_pane_infos(
        &app,
        &terminal_runtimes,
        area,
        false,
        crate::kitty_graphics::HostCellSize::default(),
    );
    let info = &infos[0];

    assert_eq!(info.rect, area);
    assert_eq!(info.scrollbar_rect, None);
    assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));
}

#[tokio::test]
async fn zoomed_multi_pane_keeps_border_space() {
    let mut app = AppState::test_new();
    let mut workspace = Workspace::test_new("test");
    let focused_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.zoomed = true;
    workspace.tabs[0].runtimes.insert(
        focused_pane,
        TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
    );
    app.workspaces = vec![workspace];
    app.active = Some(0);

    let area = Rect::new(10, 3, 40, 8);
    let terminal_runtimes = TerminalRuntimeRegistry::new();
    let infos = compute_pane_infos(
        &app,
        &terminal_runtimes,
        area,
        false,
        crate::kitty_graphics::HostCellSize::default(),
    );
    let info = &infos[0];

    assert_eq!(info.id, focused_pane);
    assert_eq!(info.rect, area);
    assert_eq!(info.scrollbar_rect, None);
    assert_eq!(info.inner_rect, Rect::new(11, 4, 37, 6));
}

#[tokio::test]
async fn tiny_pane_does_not_reserve_scrollbar_gutter() {
    let mut app = AppState::test_new();
    let mut workspace = Workspace::test_new("test");
    let root_pane = workspace.tabs[0].root_pane;
    workspace.tabs[0].runtimes.insert(
        root_pane,
        TerminalRuntime::test_with_scrollback_bytes(4, 8, 1024, b"ready\n"),
    );
    app.workspaces = vec![workspace];
    app.active = Some(0);

    let area = Rect::new(10, 3, 4, 8);
    let terminal_runtimes = TerminalRuntimeRegistry::new();
    let infos = compute_pane_infos(
        &app,
        &terminal_runtimes,
        area,
        false,
        crate::kitty_graphics::HostCellSize::default(),
    );
    let info = &infos[0];

    assert_eq!(info.rect, area);
    assert_eq!(info.scrollbar_rect, None);
    assert_eq!(info.inner_rect, area);
}

#[tokio::test]
async fn pane_scrollbar_setting_controls_reserved_column() {
    let mut app = AppState::test_new();
    let mut workspace = Workspace::test_new("test");
    let root_pane = workspace.tabs[0].root_pane;
    workspace.tabs[0].runtimes.insert(
        root_pane,
        TerminalRuntime::test_with_scrollback_bytes(
            40,
            8,
            1024,
            b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
        ),
    );
    app.workspaces = vec![workspace];
    app.active = Some(0);

    let area = Rect::new(10, 3, 40, 8);
    let terminal_runtimes = TerminalRuntimeRegistry::new();
    let infos = compute_pane_infos(
        &app,
        &terminal_runtimes,
        area,
        false,
        crate::kitty_graphics::HostCellSize::default(),
    );
    let info = &infos[0];

    assert_eq!(info.rect, area);
    assert_eq!(info.scrollbar_rect, Some(Rect::new(49, 3, 1, 8)));
    assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));

    app.pane_scrollbars = false;
    let infos = compute_pane_infos(
        &app,
        &terminal_runtimes,
        area,
        false,
        crate::kitty_graphics::HostCellSize::default(),
    );
    let info = &infos[0];

    assert_eq!(info.rect, area);
    assert_eq!(info.scrollbar_rect, None);
    assert_eq!(info.inner_rect, area);
}
