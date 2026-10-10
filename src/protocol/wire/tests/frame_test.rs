use super::*;

#[test]
fn server_frame_roundtrip_nontrivial() {
    // Build a 3×2 frame with varied styles (≥2×2).
    let frame = FrameData {
        cells: vec![
            CellData {
                symbol: "H".into(),
                fg: color_to_u32(Color::Red),
                bg: color_to_u32(Color::Black),
                modifier: Modifier::BOLD.bits(),
                skip: false,
                hyperlink: None,
            },
            CellData {
                symbol: "i".into(),
                fg: color_to_u32(Color::Green),
                bg: color_to_u32(Color::Reset),
                modifier: Modifier::ITALIC.bits(),
                skip: false,
                hyperlink: None,
            },
            CellData {
                symbol: "!".into(),
                fg: color_to_u32(Color::Rgb(255, 128, 0)),
                bg: color_to_u32(Color::Indexed(220)),
                modifier: (Modifier::BOLD | Modifier::UNDERLINED).bits(),
                skip: false,
                hyperlink: Some(0),
            },
            CellData {
                symbol: " ".into(),
                fg: color_to_u32(Color::Reset),
                bg: color_to_u32(Color::Reset),
                modifier: Modifier::empty().bits(),
                skip: true,
                hyperlink: None,
            },
            CellData {
                symbol: "→".into(), // multi-byte grapheme
                fg: color_to_u32(Color::Cyan),
                bg: color_to_u32(Color::Blue),
                modifier: Modifier::REVERSED.bits(),
                skip: false,
                hyperlink: None,
            },
            CellData {
                symbol: "🦀".into(), // emoji, wide grapheme cluster
                fg: color_to_u32(Color::Yellow),
                bg: color_to_u32(Color::Magenta),
                modifier: Modifier::empty().bits(),
                skip: false,
                hyperlink: None,
            },
        ],
        width: 3,
        height: 2,
        cursor: Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 6,
        }),
        hyperlinks: vec!["https://example.com".to_owned()],
        graphics: Vec::new(),
    };
    let msg = ServerMessage::PaneSurface(PaneSurfaceFrame {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        surface_revision: 1,
        frame: frame.clone(),
        panes: Vec::new(),
        splits: Vec::new(),
        graphics: SurfaceGraphicsScene::default(),
    });
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
    match decoded {
        ServerMessage::PaneSurface(surface) => {
            assert_eq!(surface.frame.cells[2].hyperlink, Some(0));
            assert_eq!(
                surface.frame.hyperlinks,
                vec!["https://example.com".to_owned()]
            );
        }
        other => panic!("expected pane surface, got {other:?}"),
    }
}

#[test]
fn pane_surface_patch_roundtrip() {
    let msg = ServerMessage::PaneSurfacePatch(PaneSurfacePatch {
        boot_id: "boot-1".into(),
        projection_revision: 3,
        base_surface_revision: 7,
        surface_revision: 8,
        rows: vec![PaneSurfacePatchRow {
            x: 2,
            y: 4,
            cells: vec![CellData {
                symbol: "x".into(),
                fg: 1,
                bg: 2,
                modifier: 3,
                skip: false,
                hyperlink: None,
            }],
        }],
        panes: Vec::new(),
        cursor: Some(CursorState {
            x: 2,
            y: 4,
            visible: true,
            shape: 2,
        }),
    });
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn client_shell_graphics_payload_roundtrip() {
    let key = SurfaceGraphicsAssetKey {
        source: SurfaceGraphicsSource::Terminal {
            target: SurfaceGraphicsTarget::Pane {
                pane_id: "w1:p1".into(),
            },
            image_id: 7,
        },
        image_width: 2,
        image_height: 1,
        format: SurfaceGraphicsFormat::Rgba,
        data_len: 8,
        data_fingerprint: 42,
    };
    let message = ServerMessage::PaneSurface(PaneSurfaceFrame {
        boot_id: "boot-1".into(),
        projection_revision: 2,
        surface_revision: 3,
        frame: FrameData {
            cells: Vec::new(),
            width: 0,
            height: 0,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        },
        panes: Vec::new(),
        splits: Vec::new(),
        graphics: SurfaceGraphicsScene {
            assets: vec![SurfaceGraphicsAsset {
                key: key.clone(),
                data: vec![255, 0, 0, 255, 0, 255, 0, 255],
            }],
            placements: vec![SurfaceGraphicsPlacement {
                asset: key,
                logical_placement_id: 9,
                x: 1,
                y: 2,
                cols: 2,
                rows: 1,
                source_x: 0,
                source_y: 0,
                source_width: 2,
                source_height: 1,
                x_offset: 0,
                y_offset: 0,
                z: -1,
                scrollback_offset: 0,
            }],
        },
    });
    let encoded = bincode::serde::encode_to_vec(&message, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(message, decoded);
}

#[test]
fn client_shell_snapshot_roundtrip() {
    let msg = ServerMessage::ClientShellSnapshot(Box::new(ClientShellSnapshot {
        boot_id: "boot-1".into(),
        revision: 1,
        config_diagnostic: Some("endpoint config warning".into()),
        focused_workspace_id: Some("w1".into()),
        focused_tab_id: Some("w1:t1".into()),
        focused_pane_id: Some("w1:p1".into()),
        agent_view_label: None,
        agent_order: Vec::new(),
        workspaces: vec![ClientShellWorkspace {
            workspace_id: "w1".into(),
            active_tab_id: "w1:t1".into(),
            new_workspace_cwd: "/tmp".into(),
            number: 1,
            label: "shell".into(),
            custom_label: false,
            tokens: Vec::new(),
            focused: true,
            agent_status: crate::protocol::api::schema::AgentStatus::Idle,
        }],
        tabs: vec![ClientShellTab {
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            number: 1,
            label: "main".into(),
            custom_label: true,
            zoomed: false,
            focused: true,
            agent_status: crate::protocol::api::schema::AgentStatus::Idle,
        }],
        panes: vec![ClientShellPane {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            tab_id: "w1:t1".into(),
            label: None,
            cwd: Some("/repo".into()),
            foreground_cwd: Some("/repo".into()),
            focused: true,
            right_click_passthrough: false,
        }],
        agents: Vec::new(),
    }));
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn server_graphics_roundtrip() {
    let msg = ServerMessage::Graphics {
        bytes: b"\x1b_Ga=d,d=A,q=2;\x1b\\".to_vec(),
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn server_terminal_frame_roundtrip() {
    let msg = ServerMessage::Terminal(TerminalFrame {
        seq: 7,
        width: 120,
        height: 40,
        full: false,
        bytes: b"\x1b[1;1Hhello".to_vec(),
    });
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn direct_graphics_messages_roundtrip() {
    let client = ClientMessage::GraphicsTransmissionResult {
        transfer_id: 7,
        image_id: 42,
        success: false,
    };
    let encoded = bincode::serde::encode_to_vec(&client, bincode::config::standard()).unwrap();
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(client, decoded);

    let server = ServerMessage::GraphicsFile {
        path: "/run/user/1000/bus/source/frame".into(),
        expected_len: 4,
        image_id: 42,
        transfer_id: 7,
        leading: b"\x1b[2;3H".to_vec(),
        control: "a=T,f=32,i=42,q=0".into(),
        surface_asset: None,
    };
    let encoded = bincode::serde::encode_to_vec(&server, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(server, decoded);
}

#[test]
#[allow(deprecated)]
fn frame_data_roundtrip_through_ratatui_buffer() {
    let area = ratatui::layout::Rect::new(0, 0, 5, 3);
    let mut buffer = ratatui::buffer::Buffer::filled(area, ratatui::buffer::Cell::new(" "));

    // Write some styled content.
    buffer.cell_mut((0, 0)).unwrap().set_symbol("H");
    buffer.cell_mut((0, 0)).unwrap().fg = Color::Red;
    buffer.cell_mut((0, 0)).unwrap().modifier = Modifier::BOLD;

    buffer.cell_mut((1, 0)).unwrap().set_symbol("i");
    buffer.cell_mut((1, 0)).unwrap().fg = Color::Green;
    buffer.cell_mut((1, 0)).unwrap().modifier = Modifier::ITALIC;

    buffer.cell_mut((2, 0)).unwrap().set_symbol("!");
    buffer.cell_mut((2, 0)).unwrap().fg = Color::Rgb(255, 128, 0);
    buffer.cell_mut((2, 0)).unwrap().bg = Color::Indexed(220);
    buffer
        .cell_mut((3, 0))
        .unwrap()
        .set_diff_option(ratatui::buffer::CellDiffOption::Skip);
    buffer.cell_mut((4, 0)).unwrap().set_skip(true);

    let cursor = CursorState {
        x: 1,
        y: 0,
        visible: true,
        shape: 0,
    };
    let frame = FrameData::from_ratatui_buffer(&buffer, Some(cursor.clone()));

    // Verify frame dimensions.
    assert_eq!(frame.width, 5);
    assert_eq!(frame.height, 3);
    assert_eq!(frame.cells.len(), 15);
    assert_eq!(frame.cursor, Some(cursor));

    // Verify specific cells survived the conversion.
    assert_eq!(frame.cells[0].symbol, "H");
    assert_eq!(frame.cells[0].fg, color_to_u32(Color::Red));
    assert_eq!(frame.cells[0].modifier, Modifier::BOLD.bits());

    assert_eq!(frame.cells[1].symbol, "i");
    assert_eq!(frame.cells[1].fg, color_to_u32(Color::Green));
    assert_eq!(frame.cells[1].modifier, Modifier::ITALIC.bits());

    assert_eq!(frame.cells[2].symbol, "!");
    assert_eq!(frame.cells[2].fg, color_to_u32(Color::Rgb(255, 128, 0)));
    assert_eq!(frame.cells[2].bg, color_to_u32(Color::Indexed(220)));
    assert!(frame.cells[3].skip);
    assert!(frame.cells[4].skip);

    let with_links = FrameData::from_ratatui_buffer_with_hyperlinks(
        &buffer,
        None,
        &[((1, 0), "i".to_owned(), "https://example.com".to_owned())],
    );
    assert_eq!(with_links.cells[1].hyperlink, Some(0));
    assert_eq!(
        with_links.hyperlinks,
        vec!["https://example.com".to_owned()]
    );

    // Convert back to ratatui buffer and compare.
    let restored = frame.to_ratatui_buffer().expect("should reconstruct");
    assert_eq!(restored.area, area);
    assert_eq!(restored.cell((0, 0)).unwrap().symbol(), "H");
    assert_eq!(restored.cell((0, 0)).unwrap().fg, Color::Red);
    assert_eq!(restored.cell((0, 0)).unwrap().modifier, Modifier::BOLD);
    assert_eq!(restored.cell((1, 0)).unwrap().symbol(), "i");
    assert_eq!(restored.cell((2, 0)).unwrap().symbol(), "!");
    assert_eq!(restored.cell((2, 0)).unwrap().fg, Color::Rgb(255, 128, 0));
    assert_eq!(
        restored.cell((3, 0)).unwrap().diff_option,
        ratatui::buffer::CellDiffOption::Skip
    );
    assert_eq!(
        restored.cell((4, 0)).unwrap().diff_option,
        ratatui::buffer::CellDiffOption::Skip
    );
}

#[test]
fn frame_data_rejects_mismatched_cell_count() {
    let frame = FrameData {
        cells: vec![
            CellData {
                symbol: "X".into(),
                fg: 0,
                bg: 0,
                modifier: 0,
                skip: false,
                hyperlink: None,
            };
            5
        ], // 5 cells but 3×2 = 6 expected
        width: 3,
        height: 2,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    assert!(frame.to_ratatui_buffer().is_none());
}

#[test]
fn frame_replacement_preserves_underline_style_on_unmodified_cells() {
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 2, 1));
    let cell = buffer.cell_mut((0, 0)).unwrap();
    cell.set_symbol("x");
    cell.modifier = modifier_with_underline_style(Modifier::UNDERLINED, 3);
    let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
    assert_eq!(underline_style_from_modifier(frame.cells[0].modifier), 3);

    let mut composed = frame.to_ratatui_buffer().unwrap();
    composed.cell_mut((1, 0)).unwrap().bg = Color::Red;
    frame.replace_from_ratatui_buffer_preserving_effects(&composed, None);

    assert_eq!(frame.cells[0].symbol, "x");
    assert_eq!(
        underline_style_from_modifier(frame.cells[0].modifier),
        3,
        "an overlay changing another cell must preserve the pane's curly underline"
    );
    assert_eq!(frame.cells[1].bg, color_to_u32(Color::Red));
}
