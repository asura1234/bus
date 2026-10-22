use super::*;
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
fn overlay_label_shows_cpu_memory_and_fps() {
    let sample = ContextSample {
        cpu_pct: 12.34,
        rss_mb: 92.4,
        load1: 3.0,
        sys_cpu_pct: 40.0,
    };
    assert_eq!(
        overlay_label(Some(sample), Some(29.6), None),
        " CPU 12.3% MEM 92MB FPS 30 \n AGENTS n/a "
    );
    assert_eq!(
        overlay_label(None, None, None),
        " CPU n/a MEM n/a FPS n/a \n AGENTS n/a "
    );
    assert_eq!(
        overlay_label(None, None, Some(&BusState::default())),
        " CPU n/a MEM n/a FPS n/a \n AGENTS 0 running / 0 live "
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
fn an_on_overlay_refreshes_once_a_second_with_fps_and_stamps_the_top_right() {
    let meter = OverlayMeter::new();
    let start = Instant::now();
    meter.set_on(true, start, None);
    assert!(meter.is_on());
    for _ in 0..30 {
        meter.note_frame();
    }
    assert!(
        !meter.tick(start + Duration::from_millis(500), None, None),
        "not yet a second"
    );
    assert!(
        meter.tick(start + Duration::from_secs(1), None, None),
        "a new second repaints"
    );
    assert!(!meter.tick(start + Duration::from_millis(1_100), None, None));

    let mut frame = blank_frame(80, 3);
    meter.stamp(&mut frame);
    let top = row(&frame, 0);
    assert!(top.ends_with(" CPU n/a MEM n/a FPS 30 "), "{top:?}");
    assert!(row(&frame, 1).ends_with(" AGENTS n/a "));
    assert_eq!(row(&frame, 2).trim(), "", "only the top two rows change");
    let start_col = 80 - " CPU n/a MEM n/a FPS 30 ".len();
    assert_ne!(
        frame.cells[start_col].bg, 2,
        "the label has its own background"
    );

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

#[test]
fn overlay_counts_shown_status_across_all_rooms_and_refreshes_after_turns() {
    use crate::messaging::model::{BusState, Provider, RuntimeStatus};

    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let first = state.create_room("first").unwrap();
    let second = state.create_room("second").unwrap();
    let mut working = Vec::new();
    for (room, name, status) in [
        (master, "orchestrator", RuntimeStatus::Working),
        (first, "alpha", RuntimeStatus::Working),
        (second, "beta", RuntimeStatus::Working),
        (first, "idle", RuntimeStatus::Idle),
        (second, "launching", RuntimeStatus::Launching),
        (second, "blocked", RuntimeStatus::Blocked),
        (first, "unavailable", RuntimeStatus::Unavailable),
        (first, "dialog", RuntimeStatus::Working),
    ] {
        let id = state
            .create_agent(room, name, Provider::ClaudeCode, "/scratch".into(), None)
            .unwrap();
        state.observe_status(id, status, 1).unwrap();
        if name == "dialog" {
            state
                .observe_dialog(id, true, state.agent(id).unwrap().observed_at_ms)
                .unwrap();
        } else if status == RuntimeStatus::Working {
            working.push(id);
        }
    }
    assert!(overlay_label(None, None, Some(&state)).ends_with("AGENTS 3 running / 7 live "));

    let meter = OverlayMeter::new();
    let start = Instant::now();
    meter.set_on(true, start, None);
    assert!(meter.tick(start + Duration::from_secs(1), None, Some(&state)));
    let mut frame = blank_frame(100, 2);
    meter.stamp(&mut frame);
    assert!(row(&frame, 1).ends_with("AGENTS 3 running / 7 live "));

    for id in working {
        state.observe_status(id, RuntimeStatus::Idle, 2).unwrap();
    }
    assert!(meter.tick(start + Duration::from_secs(2), None, Some(&state)));
    meter.stamp(&mut frame);
    assert!(row(&frame, 1).ends_with("AGENTS 0 running / 7 live "));
}

#[test]
fn an_overlay_immediately_shows_snapshot_agent_counts_when_enabled() {
    use crate::messaging::model::Provider;

    let mut state = BusState::default();
    let room = state.ensure_master_room();
    for (name, status) in [
        ("working", RuntimeStatus::Working),
        ("idle", RuntimeStatus::Idle),
    ] {
        let id = state
            .create_agent(room, name, Provider::ClaudeCode, "/scratch".into(), None)
            .unwrap();
        state.observe_status(id, status, 1).unwrap();
    }
    let meter = OverlayMeter::new();
    meter.set_on(true, Instant::now(), Some(&state));
    let mut frame = blank_frame(100, 2);
    meter.stamp(&mut frame);
    let top = row(&frame, 0);
    assert!(top.contains("FPS n/a"), "{top:?}");
    assert!(row(&frame, 1).ends_with(" AGENTS 1 running / 2 live "));
}

#[test]
fn an_overlay_tick_without_state_marks_agent_counts_unavailable() {
    let meter = OverlayMeter::new();
    let start = Instant::now();
    meter.set_on(true, start, Some(&BusState::default()));
    let mut frame = blank_frame(100, 2);
    meter.stamp(&mut frame);
    assert!(row(&frame, 1).ends_with("AGENTS 0 running / 0 live "));

    meter.note_frame();
    assert!(meter.tick(start + Duration::from_secs(1), None, None));
    meter.stamp(&mut frame);
    let top = row(&frame, 0);
    assert!(top.ends_with(" FPS 1 "), "{top:?}");
    assert!(row(&frame, 1).ends_with(" AGENTS n/a "));
}

fn overlay_state(running: usize, live: usize) -> BusState {
    use crate::messaging::model::Provider;

    let mut state = BusState::default();
    let room = state.ensure_master_room();
    for index in 0..live {
        let id = state
            .create_agent(
                room,
                &format!("agent-{index}"),
                Provider::ClaudeCode,
                "/scratch".into(),
                None,
            )
            .unwrap();
        let status = if index < running {
            RuntimeStatus::Working
        } else {
            RuntimeStatus::Idle
        };
        state.observe_status(id, status, 1).unwrap();
    }
    state
}

#[test]
fn overlay_renders_metrics_and_agents_on_two_right_aligned_rows() {
    let meter = OverlayMeter::new();
    let start = Instant::now();
    let state = overlay_state(11, 28);
    meter.set_on(true, start, Some(&state));
    for _ in 0..9 {
        meter.note_frame();
    }
    let sample = ContextSample {
        cpu_pct: 48.1,
        rss_mb: 84.0,
        load1: 3.0,
        sys_cpu_pct: 40.0,
    };
    assert!(meter.tick(start + Duration::from_secs(1), Some(sample), Some(&state)));
    let mut frame = blank_frame(80, 3);
    meter.stamp(&mut frame);
    assert_eq!(
        row(&frame, 0),
        format!("{:>80}", " CPU 48.1% MEM 84MB FPS 9 ")
    );
    assert_eq!(
        row(&frame, 1),
        format!("{:>80}", " AGENTS 11 running / 28 live ")
    );
    assert_eq!(row(&frame, 2), " ".repeat(80));
}

#[test]
fn overlay_box_width_fits_the_wider_row_and_highlights_both_rows() {
    let state = overlay_state(11, 28);
    for (sample, state, lines) in [
        (
            Some(ContextSample {
                cpu_pct: 48.1,
                rss_mb: 84.0,
                load1: 3.0,
                sys_cpu_pct: 40.0,
            }),
            Some(&state),
            [
                " CPU 48.1% MEM 84MB FPS 0 ",
                " AGENTS 11 running / 28 live ",
            ],
        ),
        (
            Some(ContextSample {
                cpu_pct: 1234.5,
                rss_mb: 12345.0,
                load1: 3.0,
                sys_cpu_pct: 40.0,
            }),
            Some(&state),
            [
                " CPU 1234.5% MEM 12345MB FPS 0 ",
                " AGENTS 11 running / 28 live ",
            ],
        ),
        (None, None, [" CPU n/a MEM n/a FPS 0 ", " AGENTS n/a "]),
    ] {
        let meter = OverlayMeter::new();
        let start = Instant::now();
        meter.set_on(true, start, state);
        assert!(meter.tick(start + Duration::from_secs(1), sample, state));
        let mut frame = blank_frame(80, 3);
        let before = frame.clone();
        meter.stamp(&mut frame);
        let box_width = lines.iter().map(|line| line.len()).max().unwrap();
        let start_col = 80 - box_width;
        for (y, line) in lines.iter().enumerate() {
            let cells = &frame.cells[y * 80..(y + 1) * 80];
            let highlighted: Vec<_> = cells
                .iter()
                .enumerate()
                .filter(|(_, cell)| cell.bg != 2)
                .map(|(x, _)| x)
                .collect();
            assert_eq!(
                highlighted,
                (start_col..80).collect::<Vec<_>>(),
                "row {y}: {line}"
            );
            assert_eq!(row(&frame, y as u16), format!("{line:>80}"));
            for cell in &cells[start_col..] {
                assert_eq!(
                    cell.fg,
                    crate::protocol::wire::color_to_u32(ratatui::style::Color::Black)
                );
                assert_eq!(
                    cell.bg,
                    crate::protocol::wire::color_to_u32(ratatui::style::Color::Yellow)
                );
            }
            assert_eq!(
                &cells[..start_col],
                &before.cells[y * 80..y * 80 + start_col]
            );
        }
        assert_eq!(&frame.cells[160..], &before.cells[160..]);
    }
}
