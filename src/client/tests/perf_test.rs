use super::*;
use crate::messaging::model::{Provider, RuntimeStatus};
use crate::protocol::wire::{CellData, FrameData};
use crate::utils::perf::ContextSample;
use std::time::{Duration, Instant};

fn blank_frame(width: u16, height: u16) -> FrameData {
    FrameData {
        cells: vec![
            CellData {
                symbol: " ".into(),
                fg: 1,
                bg: 2,
                modifier: 0,
                skip: false,
                hyperlink: None,
            };
            usize::from(width) * usize::from(height)
        ],
        width,
        height,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    }
}

fn row(frame: &FrameData, y: u16) -> String {
    let start = usize::from(y) * usize::from(frame.width);
    frame.cells[start..start + usize::from(frame.width)]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect()
}

#[test]
fn overlay_label_shows_only_cpu_memory_and_fps() {
    let sample = ContextSample {
        cpu_pct: 12.34,
        rss_mb: 92.4,
        load1: 3.0,
        sys_cpu_pct: 40.0,
    };
    assert_eq!(
        overlay_label(Some(sample), Some(29.6), None),
        " CPU 12.3% MEM 92MB FPS 30 "
    );
    assert_eq!(overlay_label(None, None, None), " CPU n/a MEM n/a FPS n/a ");
    assert_eq!(
        overlay_label(None, None, Some(&BusState::default())),
        " CPU n/a MEM n/a FPS n/a "
    );
}

#[test]
fn an_off_overlay_leaves_frames_alone_and_counts_nothing() {
    let meter = OverlayMeter::new();
    let mut frame = blank_frame(40, 3);
    let before = frame.clone();
    meter.note_frame();
    meter.stamp(&mut frame);
    assert_eq!(frame, before);
    assert!(!meter.tick(Instant::now() + Duration::from_secs(5), None, None));
    assert_eq!(meter.frames_since_sample(), 0);
}

#[test]
fn an_on_overlay_refreshes_once_a_second_and_stamps_only_the_top_row() {
    let meter = OverlayMeter::new();
    let start = Instant::now();
    meter.set_on(true, start, None);
    for _ in 0..30 {
        meter.note_frame();
    }
    assert!(!meter.tick(start + Duration::from_millis(500), None, None));
    assert!(meter.tick(start + Duration::from_secs(1), None, None));
    assert!(!meter.tick(start + Duration::from_millis(1_100), None, None));

    let mut frame = blank_frame(80, 3);
    meter.stamp(&mut frame);
    assert!(row(&frame, 0).ends_with(" CPU n/a MEM n/a FPS 30 "));
    assert_eq!(row(&frame, 1).trim(), "");
    assert_eq!(row(&frame, 2).trim(), "");
    let start_col = 80 - " CPU n/a MEM n/a FPS 30 ".len();
    assert_ne!(frame.cells[start_col].bg, 2);

    meter.set_on(false, start, None);
    let mut cleared = blank_frame(40, 3);
    meter.stamp(&mut cleared);
    assert_eq!(row(&cleared, 0).trim(), "");
}

#[test]
fn a_narrow_frame_gets_a_clipped_label_without_panicking() {
    let meter = OverlayMeter::new();
    meter.set_on(true, Instant::now(), None);
    let mut frame = blank_frame(10, 1);
    meter.stamp(&mut frame);
    assert_eq!(row(&frame, 0).chars().count(), 10);
}

#[test]
fn agent_metrics_never_return_to_the_overlay_after_state_refreshes() {
    let mut state = BusState::default();
    let room = state.ensure_master_room();
    for (name, status) in [
        ("working", RuntimeStatus::Working),
        ("blocked", RuntimeStatus::Blocked),
        ("idle", RuntimeStatus::Idle),
    ] {
        let id = state
            .create_agent(room, name, Provider::ClaudeCode, "/scratch".into(), None)
            .unwrap();
        state.observe_status(id, status, 1).unwrap();
    }
    let meter = OverlayMeter::new();
    let start = Instant::now();
    meter.set_on(true, start, Some(&state));
    assert!(meter.tick(start + Duration::from_secs(1), None, Some(&state)));
    let mut frame = blank_frame(100, 3);
    meter.stamp(&mut frame);
    assert!(row(&frame, 0).contains("FPS 0"));
    assert_eq!(row(&frame, 1).trim(), "");
    assert!(!frame.cells.iter().any(|cell| cell.symbol == "A"));
}

#[test]
fn overlay_box_width_and_highlight_fit_the_single_metrics_row() {
    let meter = OverlayMeter::new();
    let start = Instant::now();
    meter.set_on(true, start, None);
    assert!(meter.tick(
        start + Duration::from_secs(1),
        Some(ContextSample {
            cpu_pct: 48.1,
            rss_mb: 84.0,
            load1: 3.0,
            sys_cpu_pct: 40.0,
        }),
        None
    ));
    let mut frame = blank_frame(80, 3);
    let before = frame.clone();
    meter.stamp(&mut frame);
    let line = " CPU 48.1% MEM 84MB FPS 0 ";
    let start_col = 80 - line.len();
    assert_eq!(row(&frame, 0), format!("{line:>80}"));
    assert_eq!(row(&frame, 1), " ".repeat(80));
    for cell in &frame.cells[start_col..80] {
        assert_eq!(
            cell.fg,
            crate::protocol::wire::color_to_u32(ratatui::style::Color::Black)
        );
        assert_eq!(
            cell.bg,
            crate::protocol::wire::color_to_u32(ratatui::style::Color::Yellow)
        );
    }
    assert_eq!(&frame.cells[..start_col], &before.cells[..start_col]);
}

/// Per-frame cost of the overlay: `cargo test --release --bin bus overlay_cost_profile -- --ignored --nocapture`.
#[test]
#[ignore = "profile, not a check"]
fn overlay_cost_profile() {
    const FRAMES: u32 = 200_000;
    let per_frame = |meter: &OverlayMeter, count_fps: bool| {
        let mut frame = blank_frame(140, 40);
        let started = Instant::now();
        for _ in 0..FRAMES {
            if count_fps {
                meter.note_frame();
            }
            meter.stamp(&mut frame);
            std::hint::black_box(&frame);
        }
        started.elapsed().as_nanos() / u128::from(FRAMES)
    };
    let off = OverlayMeter::new();
    let on = OverlayMeter::new();
    on.set_on(true, Instant::now(), None);
    println!(
        "overlay ns/frame: off={} on_without_fps={} on_with_fps={}",
        per_frame(&off, true),
        per_frame(&on, false),
        per_frame(&on, true)
    );
    let started = Instant::now();
    for _ in 0..FRAMES {
        on.note_frame();
    }
    println!(
        "fps counter ns/frame: {}",
        started.elapsed().as_nanos() / u128::from(FRAMES)
    );
}
