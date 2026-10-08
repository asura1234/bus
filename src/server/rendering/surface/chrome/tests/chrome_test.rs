fn render_view_pane_borders(
    app: &AppState,
    ws: &Workspace,
    split_borders: &[crate::layout::SplitBorder],
    frame: &mut Frame,
) {
    render_pane_borders(app, ws, &app.view.pane_infos, split_borders, frame);
}

#[test]
fn pane_border_title_trims_and_truncates() {
    assert_eq!(
        pane_border_title(" claude ", 20).as_deref(),
        Some(" claude ")
    );
    assert_eq!(pane_border_title("", 20), None);
    assert_eq!(pane_border_title("abcdef", 8).as_deref(), Some(" abc… "));
    assert_eq!(pane_border_title("abcdef", 4), None);
}

#[test]
fn pane_border_title_truncates_cjk_by_display_width() {
    let title = pane_border_title("1 模块组织（已定）", 12).unwrap();

    assert_eq!(title, " 1 模块… ");
    assert!(display_width(title.as_str()) <= 10);
}

#[test]
fn pane_border_renderer_places_adjacent_cjk_by_display_width() {
    let mut app = AppState::test_new();
    app.view.terminal_area = Rect::new(0, 0, 12, 3);
    let ws = Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    app.view.pane_infos = vec![PaneInfo {
        id: pane_id,
        rect: Rect::new(0, 0, 12, 3),
        inner_rect: Rect::default(),
        scrollbar_rect: None,
        borders: Borders::ALL,
        is_focused: false,
    }];

    let terminal_id = ws.tabs[0].panes[&pane_id].attached_terminal_id.clone();
    let mut terminal_state = TerminalState::new(terminal_id.clone(), "/tmp".into());
    terminal_state.set_manual_label("1 模块组织（已定）".into());
    app.terminals.insert(terminal_id, terminal_state);

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(12, 3)).unwrap();
    terminal
        .draw(|frame| render_view_pane_borders(&app, &ws, &[], frame))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(4, 0)].symbol(), "模");
    assert_eq!(buffer[(5, 0)].symbol(), " ");
    assert_eq!(buffer[(6, 0)].symbol(), "块");
}

#[test]
fn default_horizontal_split_uses_one_shared_divider_column() {
    let mut workspace = Workspace::test_new("test");
    let root = workspace.tabs[0].root_pane;
    let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(root);

    let infos = apply_pane_chrome(
        workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
        PaneBordersConfig::Auto,
        false,
        true,
    );
    let left = infos.iter().find(|info| info.id == root).unwrap();
    let right = infos.iter().find(|info| info.id == right).unwrap();

    assert_eq!(left.rect.x + left.rect.width, right.rect.x);
    assert!(!left.borders.contains(Borders::RIGHT));
    assert!(right.borders.contains(Borders::LEFT));
}

#[test]
fn default_vertical_split_uses_one_shared_divider_row() {
    let mut workspace = Workspace::test_new("test");
    let root = workspace.tabs[0].root_pane;
    let bottom = workspace.test_split(ratatui::layout::Direction::Vertical);
    workspace.tabs[0].layout.focus_pane(root);

    let infos = apply_pane_chrome(
        workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
        PaneBordersConfig::Auto,
        false,
        true,
    );
    let top = infos.iter().find(|info| info.id == root).unwrap();
    let bottom = infos.iter().find(|info| info.id == bottom).unwrap();

    assert_eq!(top.rect.y + top.rect.height, bottom.rect.y);
    assert!(!top.borders.contains(Borders::BOTTOM));
    assert!(bottom.borders.contains(Borders::TOP));
}

#[test]
fn disabled_outer_borders_keep_only_shared_pane_dividers() {
    let mut workspace = Workspace::test_new("test");
    let root = workspace.tabs[0].root_pane;
    let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(root);

    let infos = apply_pane_chrome(
        workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
        PaneBordersConfig::Auto,
        false,
        false,
    );
    let left = infos.iter().find(|info| info.id == root).unwrap();
    let right = infos.iter().find(|info| info.id == right).unwrap();

    assert_eq!(left.borders, Borders::NONE);
    assert_eq!(right.borders, Borders::LEFT);
}

#[test]
fn pane_gaps_keep_independent_bordered_panes() {
    let mut workspace = Workspace::test_new("test");
    let root = workspace.tabs[0].root_pane;
    let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(root);

    let infos = apply_pane_chrome(
        workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
        PaneBordersConfig::Auto,
        true,
        true,
    );
    let left = infos.iter().find(|info| info.id == root).unwrap();
    let right = infos.iter().find(|info| info.id == right).unwrap();

    assert_eq!(left.rect.x + left.rect.width, right.rect.x);
    assert_eq!(left.borders, Borders::ALL);
    assert_eq!(right.borders, Borders::ALL);
}

#[test]
fn borderless_pane_gaps_add_one_empty_cell_between_panes() {
    let mut workspace = Workspace::test_new("test");
    let root = workspace.tabs[0].root_pane;
    let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(root);

    let infos = apply_pane_chrome(
        workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
        PaneBordersConfig::Off,
        true,
        true,
    );
    let left = infos.iter().find(|info| info.id == root).unwrap();
    let right = infos.iter().find(|info| info.id == right).unwrap();

    assert_eq!(left.rect, Rect::new(0, 0, 49, 20));
    assert_eq!(right.rect, Rect::new(50, 0, 50, 20));
    assert!(left.borders.is_empty());
    assert!(right.borders.is_empty());
}

#[test]
fn disabled_pane_borders_make_inner_rect_equal_visual_rect() {
    let mut workspace = Workspace::test_new("test");
    workspace.test_split(ratatui::layout::Direction::Horizontal);

    let infos = apply_pane_chrome(
        workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
        PaneBordersConfig::Off,
        false,
        true,
    );

    for info in infos {
        assert!(info.borders.is_empty());
        assert_eq!(pane_inner_rect(info.rect, info.borders), info.rect);
    }
}

#[test]
fn always_pane_borders_frame_lone_pane() {
    let workspace = Workspace::test_new("test");
    let area = Rect::new(0, 0, 100, 20);

    let default_infos = apply_pane_chrome(
        workspace.tabs[0].layout.panes(area),
        PaneBordersConfig::Auto,
        false,
        true,
    );
    assert_eq!(default_infos[0].borders, Borders::NONE);

    let framed_infos = apply_pane_chrome(
        workspace.tabs[0].layout.panes(area),
        PaneBordersConfig::Always,
        false,
        true,
    );
    assert_eq!(framed_infos[0].borders, Borders::ALL);

    let no_outer_infos = apply_pane_chrome(
        workspace.tabs[0].layout.panes(area),
        PaneBordersConfig::Always,
        false,
        false,
    );
    assert_eq!(no_outer_infos[0].borders, Borders::NONE);
}

#[test]
fn global_pane_border_renderer_composes_junctions_and_focus_style() {
    let mut app = AppState::test_new();
    app.view.terminal_area = Rect::new(0, 0, 4, 4);
    app.view.pane_infos = vec![
        PaneInfo {
            id: PaneId::from_raw(1),
            rect: Rect::new(0, 0, 2, 2),
            inner_rect: Rect::default(),
            scrollbar_rect: None,
            borders: Borders::TOP | Borders::LEFT,
            is_focused: true,
        },
        PaneInfo {
            id: PaneId::from_raw(2),
            rect: Rect::new(2, 0, 2, 2),
            inner_rect: Rect::default(),
            scrollbar_rect: None,
            borders: Borders::TOP | Borders::LEFT | Borders::RIGHT,
            is_focused: false,
        },
        PaneInfo {
            id: PaneId::from_raw(3),
            rect: Rect::new(0, 2, 2, 2),
            inner_rect: Rect::default(),
            scrollbar_rect: None,
            borders: Borders::TOP | Borders::LEFT | Borders::BOTTOM,
            is_focused: false,
        },
        PaneInfo {
            id: PaneId::from_raw(4),
            rect: Rect::new(2, 2, 2, 2),
            inner_rect: Rect::default(),
            scrollbar_rect: None,
            borders: Borders::ALL,
            is_focused: false,
        },
    ];
    let split_borders = vec![
        crate::layout::SplitBorder {
            pos: 2,
            direction: ratatui::layout::Direction::Horizontal,
            ratio: 0.5,
            area: Rect::new(0, 0, 4, 4),
            path: vec![],
        },
        crate::layout::SplitBorder {
            pos: 2,
            direction: ratatui::layout::Direction::Vertical,
            ratio: 0.5,
            area: Rect::new(0, 0, 4, 4),
            path: vec![false],
        },
    ];
    let ws = Workspace::test_new("test");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(4, 4)).unwrap();

    terminal
        .draw(|frame| render_view_pane_borders(&app, &ws, &split_borders, frame))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(2, 2)].symbol(), "┼");
    assert_eq!(buffer[(2, 2)].style().fg, Some(app.palette.accent));
    assert_eq!(buffer[(2, 1)].symbol(), "│");
    assert_eq!(buffer[(2, 1)].style().fg, Some(app.palette.accent));
}

#[test]
fn gapped_pane_focus_does_not_color_neighbor_border() {
    let mut app = AppState::test_new();
    app.pane_gaps = true;
    app.view.terminal_area = Rect::new(0, 0, 4, 3);
    app.view.pane_infos = vec![
        PaneInfo {
            id: PaneId::from_raw(1),
            rect: Rect::new(0, 0, 2, 3),
            inner_rect: Rect::default(),
            scrollbar_rect: None,
            borders: Borders::ALL,
            is_focused: true,
        },
        PaneInfo {
            id: PaneId::from_raw(2),
            rect: Rect::new(2, 0, 2, 3),
            inner_rect: Rect::default(),
            scrollbar_rect: None,
            borders: Borders::ALL,
            is_focused: false,
        },
    ];
    let ws = Workspace::test_new("test");
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(4, 3)).unwrap();

    terminal
        .draw(|frame| render_view_pane_borders(&app, &ws, &[], frame))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(1, 1)].style().fg, Some(app.palette.accent));
    assert_eq!(buffer[(2, 1)].style().fg, Some(app.palette.overlay0));
}
