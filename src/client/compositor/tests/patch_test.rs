use super::*;
use crate::client::compositor::tests::{snapshot, surface};
use crate::client::compositor::ClientShellConfig;
use crate::protocol::wire::{
    CellData, CursorState, PaneSurfacePatch, PaneSurfacePatchRow, SurfaceRect,
};
use crate::utils::config::Config;
use ratatui::buffer::Buffer;
use std::time::{Duration, Instant};

#[test]
fn retained_patches_preserve_the_overlay_row_without_composing() {
    // The production meter is global; keep it out of other tests' compositors.
    const CHILD: &str = "PERF_OVERLAY_PATCH_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "client::compositor::patch::tests::retained_patches_preserve_the_overlay_row_without_composing",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    for (width, height) in [(100, 4), (10, 4), (100, 1)] {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(snapshot()));
        let mut pane_surface = surface();
        pane_surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(
            &Buffer::empty(Rect::new(0, 0, width, height)),
            None,
            &[],
        );
        let pane = &mut pane_surface.panes[0];
        pane.rect = SurfaceRect {
            x: 0,
            y: 0,
            width,
            height,
        };
        pane.inner_rect = pane.rect;
        let mut updated_pane = pane.clone();
        updated_pane.content_revision = 2;
        state.set_pane_surface(pane_surface);

        let meter = &crate::client::perf::OVERLAY;
        let now = Instant::now();
        meter.set_on(true, now, None);
        assert!(meter.tick(now + Duration::from_secs(1), None, None));
        let before = state.compose(width, height).unwrap();
        let box_width = " CPU n/a MEM n/a FPS 0 ".len().min(usize::from(width));
        let start = usize::from(width) - box_width;
        let ink = CellData {
            symbol: "x".into(),
            fg: 1,
            bg: 2,
            modifier: 0,
            skip: false,
            hyperlink: None,
        };
        let mut rows: Vec<_> = (0..height)
            .map(|y| PaneSurfacePatchRow {
                x: 0,
                y,
                cells: vec![ink.clone(); usize::from(width)],
            })
            .collect();
        if start > 0 {
            rows[0].cells[start - 1].symbol = "界".into();
            rows[0].cells[start].skip = true;
        }
        let source_rows = rows.clone();
        let cursor = CursorState {
            x: width - 1,
            y: 0,
            visible: true,
            shape: 2,
        };
        let ClientPaneSurfacePatchOutcome::Applied(Some(patch)) =
            state.apply_pane_surface_patch(PaneSurfacePatch {
                boot_id: "boot-1".into(),
                projection_revision: 1,
                base_surface_revision: 1,
                surface_revision: 2,
                rows,
                panes: vec![updated_pane],
                cursor: Some(cursor),
            })
        else {
            panic!("the overlay must preserve the retained fast path");
        };

        let after = apply_composed_surface_patch(&before, patch).unwrap();
        for y in 0..1u16 {
            let offset = usize::from(y) * usize::from(width);
            assert_eq!(
                &after.cells[offset + start..offset + usize::from(width)],
                &before.cells[offset + start..offset + usize::from(width)],
                "overlay row {y} was overwritten by a retained patch"
            );
        }
        if start > 0 {
            assert_eq!(
                after.cells[start - 1].symbol,
                " ",
                "a wide glyph must not spill into the box"
            );
            assert_eq!(after.cells[0].symbol, "x", "uncovered cells still update");
        }
        if height > 2 {
            assert_eq!(
                &after.cells[usize::from(width) * 2..],
                &source_rows[2..]
                    .iter()
                    .flat_map(|row| row.cells.clone())
                    .collect::<Vec<_>>()
            );
        }
        assert!(
            !after.cursor.as_ref().is_some_and(|cursor| cursor.visible),
            "a cursor in the overlay must not paint over it"
        );
        let retained = state.pane_surface.as_ref().unwrap();
        assert_eq!(retained.surface_revision, 2);
        for row in &source_rows {
            let offset = usize::from(row.y) * usize::from(width);
            assert_eq!(
                &retained.frame.cells[offset..offset + usize::from(width)],
                &row.cells,
                "clipping must not discard pane data"
            );
        }
        let retained_cells = retained.frame.cells.clone();
        let restamped = state.compose(width, height).unwrap();
        for y in 0..1u16 {
            let offset = usize::from(y) * usize::from(width);
            assert_eq!(
                &restamped.cells[offset + start..offset + usize::from(width)],
                &before.cells[offset + start..offset + usize::from(width)],
                "the next full frame must preserve the same overlay box"
            );
        }
        assert!(!restamped
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.visible));
        meter.set_on(false, now, None);
        let restored = state.compose(width, height).unwrap();
        assert_eq!(
            restored.cells, retained_cells,
            "turning the overlay off reveals the latest pane contents"
        );
    }
}
