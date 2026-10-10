#[tokio::test]
async fn different_size_shells_receive_geometry_specific_patches_from_one_dirty_collection() {
    let mut server = test_headless_server();
    let pane_id = install_shared_view_test_runtime(&mut server);
    let (large_control, large_render) = connect_test_shell(&mut server, 7, 80, 23);
    let (small_control, small_render) = connect_test_shell(&mut server, 8, 68, 17);
    let _ = large_control.recv().expect("large snapshot");
    let _ = small_control.recv().expect("small snapshot");
    server.render_and_stream();
    let large_initial = recv_pane_surface(&large_render, "large initial surface");
    let small_initial = recv_pane_surface(&small_render, "small initial surface");
    assert_eq!(
        (large_initial.frame.width, large_initial.frame.height),
        (80, 23)
    );
    assert_eq!(
        (small_initial.frame.width, small_initial.frame.height),
        (68, 17)
    );
    assert_ne!(
        large_initial.panes[0].inner_rect,
        small_initial.panes[0].inner_rect
    );

    write_shared_test_pane(&mut server, pane_id, b"\rMIXED");
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));

    let large_patch = recv_pane_surface_patch(&large_render, "large retained patch");
    let small_patch = recv_pane_surface_patch(&small_render, "small retained patch");
    assert_eq!(
        large_patch.base_surface_revision,
        large_initial.surface_revision
    );
    assert_eq!(
        small_patch.base_surface_revision,
        small_initial.surface_revision
    );
    assert!(large_patch.rows.iter().all(|row| {
        row.x
            .saturating_add(u16::try_from(row.cells.len()).unwrap_or(u16::MAX))
            <= large_initial.frame.width
            && row.y < large_initial.frame.height
    }));
    assert!(small_patch.rows.iter().all(|row| {
        row.x
            .saturating_add(u16::try_from(row.cells.len()).unwrap_or(u16::MAX))
            <= small_initial.frame.width
            && row.y < small_initial.frame.height
    }));
    assert_eq!(large_patch.rows, small_patch.rows);
    assert_ne!(
        large_patch.panes[0].inner_rect,
        small_patch.panes[0].inner_rect
    );
    assert!(frame_text(
        &server.clients[&7]
            .render_state
            .last_pane_surface()
            .expect("large retained surface")
            .frame
    )
    .contains("MIXED"));
    assert!(frame_text(
        &server.clients[&8]
            .render_state
            .last_pane_surface()
            .expect("small retained surface")
            .frame
    )
    .contains("MIXED"));

    write_shared_test_pane(&mut server, pane_id, b"\x1b[?1049hALT");
    assert!(!server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));
    server.render_and_stream();
    let large_alt = recv_pane_surface(&large_render, "large alternate-screen surface");
    let small_alt = recv_pane_surface(&small_render, "small alternate-screen surface");
    assert!(large_alt.panes[0].alternate_screen_active);
    assert!(small_alt.panes[0].alternate_screen_active);
    assert_eq!(
        large_alt.panes[0].inner_rect.width,
        large_initial.panes[0].inner_rect.width + 1
    );
    assert_eq!(
        small_alt.panes[0].inner_rect.width,
        small_initial.panes[0].inner_rect.width + 1
    );

    write_shared_test_pane(&mut server, pane_id, b"\x1b[?1049l");
    assert!(!server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));
    server.render_and_stream();
    let large_main = recv_pane_surface(&large_render, "large restored main-screen surface");
    let small_main = recv_pane_surface(&small_render, "small restored main-screen surface");
    assert!(!large_main.panes[0].alternate_screen_active);
    assert!(!small_main.panes[0].alternate_screen_active);
    assert_eq!(
        large_main.panes[0].inner_rect,
        large_initial.panes[0].inner_rect
    );
    assert_eq!(
        small_main.panes[0].inner_rect,
        small_initial.panes[0].inner_rect
    );

    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn retained_patches_only_reach_shells_viewing_the_dirty_tab() {
    let mut server = test_headless_server();
    let mut workspace = crate::server::workspaces::Workspace::test_new("divergent-retained");
    let first_pane = workspace.tabs[0].root_pane;
    let second_tab = workspace.test_add_tab(Some("second"));
    let second_pane = workspace.tabs[second_tab].root_pane;
    workspace.insert_test_runtime(
        first_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 23, b"FIRST"),
    );
    workspace.insert_test_runtime(
        second_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 23, b"SECOND"),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let second_tab_id = server.app.public_tab_id(0, second_tab).unwrap();

    let (first_control, first_render) = connect_matching_test_shell(&mut server, 7);
    let (second_control, second_render) = connect_matching_test_shell(&mut server, 8);
    let _ = first_control.recv().expect("first snapshot");
    let _ = second_control.recv().expect("second snapshot");
    assert!(server.focus_shell_client_on_tab(8, &second_tab_id));
    assert!(server.claim_shell_tab_geometry(8, false));
    assert!(
        server.pty_sources_visible_to_any_render_target(&HashSet::from([first_pane, second_pane,]))
    );
    server.render_and_stream();
    let _ = recv_pane_surface(&first_render, "first baseline");
    let _ = recv_pane_surface(&second_render, "second baseline");

    server.app.state.workspaces[0].test_runtimes[&first_pane]
        .test_process_pty_bytes(b"\rFIRST_PATCH");
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([first_pane])));
    let first_patch = recv_pane_surface_patch(&first_render, "first patch");
    assert_eq!(first_patch.panes.len(), 1);
    assert!(second_render.try_recv().is_err());

    server.app.state.workspaces[0].test_runtimes[&second_pane]
        .test_process_pty_bytes(b"\rSECOND_PATCH");
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([second_pane])));
    let second_patch = recv_pane_surface_patch(&second_render, "second patch");
    assert_eq!(second_patch.panes.len(), 1);
    assert!(first_render.try_recv().is_err());

    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn backpressured_shell_does_not_disable_retained_patches_for_responsive_peer() {
    let mut server = test_headless_server();
    let pane_id = install_shared_view_test_runtime(&mut server);
    let (responsive_control, responsive_render) = connect_matching_test_shell(&mut server, 7);
    let (slow_control, slow_render) = connect_matching_test_shell(&mut server, 8);
    let _ = responsive_control.recv().expect("responsive snapshot");
    let _ = slow_control.recv().expect("slow snapshot");
    server.render_and_stream();
    let _ = responsive_render
        .recv()
        .expect("responsive initial surface");
    let _ = slow_render.recv().expect("slow initial surface");

    let sources = HashSet::from([pane_id]);
    write_shared_test_pane(&mut server, pane_id, b"\rONE");
    assert!(server.render_retained_pane_surface_and_stream(&sources));
    assert!(matches!(
        read_server_message(responsive_render.recv().expect("responsive first patch")),
        ServerMessage::PaneSurfacePatch(_)
    ));

    write_shared_test_pane(&mut server, pane_id, b"\rTWO");
    assert!(server.render_retained_pane_surface_and_stream(&sources));
    assert!(matches!(
        read_server_message(responsive_render.recv().expect("responsive second patch")),
        ServerMessage::PaneSurfacePatch(_)
    ));
    assert_eq!(server.clients[&8].deferred_render(), DeferredRender::Full);

    write_shared_test_pane(&mut server, pane_id, b"\rTHREE");
    assert!(server.render_retained_pane_surface_and_stream(&sources));
    assert!(matches!(
        read_server_message(responsive_render.recv().expect("responsive third patch")),
        ServerMessage::PaneSurfacePatch(_)
    ));

    assert!(matches!(
        read_server_message(slow_render.recv().expect("slow queued first patch")),
        ServerMessage::PaneSurfacePatch(_)
    ));
    assert!(server.handle_server_event(ServerEvent::ClientWriterDrained { client_id: 8 }));
    server.render_and_stream();
    assert!(matches!(
        read_server_message(slow_render.recv().expect("slow full recovery surface")),
        ServerMessage::PaneSurface(_)
    ));

    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn full_render_backpressure_does_not_disable_responsive_peer_patches() {
    let mut server = test_headless_server();
    let pane_id = install_shared_view_test_runtime(&mut server);
    let (responsive_control, responsive_render) = connect_matching_test_shell(&mut server, 7);
    let (slow_control, slow_render) = connect_matching_test_shell(&mut server, 8);
    let _ = responsive_control.recv().expect("responsive snapshot");
    let _ = slow_control.recv().expect("slow snapshot");
    server.render_and_stream();
    let _ = responsive_render
        .recv()
        .expect("responsive initial surface");
    // Keep the slow client's initial surface queued, then force another full
    // replacement for both clients.
    server.clients.get_mut(&7).unwrap().request_repaint();
    server.clients.get_mut(&8).unwrap().request_repaint();
    server.app.full_redraw_pending = true;
    server.render_and_stream();
    let _ = responsive_render
        .recv()
        .expect("responsive full replacement");
    assert_eq!(server.clients[&8].deferred_render(), DeferredRender::Full);
    assert!(!server.app.full_redraw_pending);

    write_shared_test_pane(&mut server, pane_id, b"\rPATCH");
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));
    assert!(matches!(
        read_server_message(responsive_render.recv().expect("responsive retained patch")),
        ServerMessage::PaneSurfacePatch(_)
    ));

    let _ = slow_render.recv().expect("slow queued initial surface");
    assert!(server.handle_server_event(ServerEvent::ClientWriterDrained { client_id: 8 }));
    server.render_and_stream();
    assert!(matches!(
        read_server_message(slow_render.recv().expect("slow full recovery surface")),
        ServerMessage::PaneSurface(_)
    ));

    shutdown_test_runtimes(&mut server);
}

fn retained_frame_after_scrollback_matches_full_render(
    cols: u16,
    rows: u16,
    borders: crate::utils::config::PaneBordersConfig,
) {
    let mut server = test_headless_server();
    server.app.state.pane_scrollbars = true;
    server.app.state.pane_borders = borders;
    let mut workspace = crate::server::workspaces::Workspace::test_new("scrollback-retained");
    let pane_id = workspace.focused_pane_id().expect("focused pane");
    workspace.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(cols, rows, 1 << 20, b"BASE"),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::server::app_settings::Mode::Terminal;
    let (control, render) = connect_test_shell(&mut server, 7, cols, rows);
    let _ = control.recv().expect("snapshot");
    server.render_and_stream();
    let initial = recv_pane_surface(&render, "initial surface");

    let mut output = Vec::new();
    for line in 0..40 {
        output.extend_from_slice(format!("\r\nL{line}").as_bytes());
    }
    write_shared_test_pane(&mut server, pane_id, &output);
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));
    let _ = recv_pane_surface_patch(&render, "retained patch");
    let retained = server.clients[&7]
        .render_state
        .last_pane_surface()
        .expect("retained surface")
        .clone();

    server.clients.get_mut(&7).unwrap().request_repaint();
    server.render_and_stream();
    let full = recv_pane_surface(&render, "full replacement surface");
    assert_eq!(
        retained.frame.cells, full.frame.cells,
        "retained frame diverged from full render; initial panes {:?}, full panes {:?}",
        initial.panes, full.panes
    );
    assert_eq!(retained.panes, full.panes);
    assert!(
        full.panes[0]
            .scroll
            .is_some_and(|scroll| scroll.max_offset_from_bottom > 0),
        "probe must exercise a pane with scrollback: {:?}",
        full.panes
    );

    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn retained_scrollback_patch_matches_full_render_for_wide_pane() {
    retained_frame_after_scrollback_matches_full_render(
        80,
        23,
        crate::utils::config::PaneBordersConfig::Auto,
    );
}

#[tokio::test]
async fn retained_scrollback_patch_matches_full_render_for_wide_bordered_pane() {
    retained_frame_after_scrollback_matches_full_render(
        80,
        23,
        crate::utils::config::PaneBordersConfig::Always,
    );
}

#[tokio::test]
async fn retained_scrollback_patch_matches_full_render_for_narrow_bordered_pane() {
    retained_frame_after_scrollback_matches_full_render(
        6,
        10,
        crate::utils::config::PaneBordersConfig::Always,
    );
}

#[tokio::test]
async fn retained_scrollback_patch_matches_full_render_for_narrow_pane() {
    retained_frame_after_scrollback_matches_full_render(
        5,
        10,
        crate::utils::config::PaneBordersConfig::Auto,
    );
}

#[tokio::test]
async fn retained_scrollback_patch_matches_full_render_for_bordered_pane_at_gutter_threshold() {
    retained_frame_after_scrollback_matches_full_render(
        7,
        10,
        crate::utils::config::PaneBordersConfig::Always,
    );
}
