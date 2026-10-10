use std::hint::black_box;
use std::time::{Duration, Instant};

use ratatui::layout::{Direction, Rect};

use crate::client::compositor::{ClientShellConfig, ClientShellState};
use crate::protocol::kitty::HostCellSize;
use crate::protocol::wire::PaneSurfaceFrame;
use crate::server::app::{App, AppPolicy};
use crate::server::workspaces::Workspace;
use crate::terminal::TerminalRuntime;
use crate::utils::config::Config;

const COLS: u16 = 120;
const ROWS: u16 = 40;
const SAMPLE_COUNT: usize = 40;
const WARMUP_COUNT: usize = 5;
const CARDINALITIES: [usize; 3] = [1, 15, 50];
const CLIENT_CARDINALITIES: [usize; 2] = [1, 4];

#[derive(Clone, Copy)]
struct StageStats {
    median_us: u128,
    p95_us: u128,
    max_us: u128,
}

struct PipelineStats {
    server: StageStats,
    client: StageStats,
    total: StageStats,
}

struct RenderPipeline {
    app: App,
    client: ClientShellState,
    graphics_delivery: crate::server::rendering::images::DeliveryCache,
}

impl RenderPipeline {
    fn new(workspaces: Vec<Workspace>) -> Self {
        Self::with_config(workspaces, &Config::default())
    }

    fn with_config(workspaces: Vec<Workspace>, config: &Config) -> Self {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            config,
            AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        app.state.workspaces = workspaces;
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.pane_scrollbars = true;

        let mut client = ClientShellState::new(ClientShellConfig::from_config(config));
        client.set_snapshot(Box::new(crate::server::rendering::snapshot::snapshot(
            &app,
            "bench-boot",
            1,
            None,
            None,
        )));

        Self {
            app,
            client,
            graphics_delivery: crate::server::rendering::images::DeliveryCache::default(),
        }
    }

    fn render_once(&mut self) -> (Duration, Duration) {
        let surface_size = self.client.surface_size(COLS, ROWS);
        let started = Instant::now();
        let target = self.app.state.active.map(|workspace_index| {
            crate::server::rendering::surface::TabSurfaceTarget {
                workspace_index,
                tab_index: self.app.state.workspaces[workspace_index].active_tab_index(),
            }
        });
        let rendered = crate::server::rendering::snapshot::render_pane_surface(
            &mut self.app,
            target,
            Rect::new(0, 0, surface_size.cols, surface_size.rows),
            true,
            HostCellSize {
                width_px: 1,
                height_px: 1,
            },
            &self.graphics_delivery,
        );
        let server_elapsed = started.elapsed();
        self.graphics_delivery = rendered.graphics_delivery;

        let started = Instant::now();
        self.client.set_pane_surface(PaneSurfaceFrame {
            boot_id: "bench-boot".into(),
            projection_revision: 1,
            surface_revision: 0,
            frame: rendered.frame,
            panes: rendered.panes,
            splits: rendered.splits,
            graphics: rendered.graphics,
        });
        black_box(
            self.client
                .compose(COLS, ROWS)
                .expect("benchmark pipeline should compose a complete frame"),
        );
        let client_elapsed = started.elapsed();

        (server_elapsed, client_elapsed)
    }
}

fn history() -> String {
    (0..2_000).map(|line| format!("line-{line}\r\n")).collect()
}

fn runtime(history: &str) -> TerminalRuntime {
    TerminalRuntime::test_with_scrollback_bytes(COLS, ROWS, 1024 * 1024, history.as_bytes())
}

fn workspaces(workspace_count: usize) -> Vec<Workspace> {
    let history = history();
    (0..workspace_count)
        .map(|index| {
            let mut workspace = Workspace::test_new(&format!("bench-{}", index + 1));
            let root_pane = workspace.tabs[0].root_pane;
            workspace.insert_test_runtime(root_pane, runtime(&history));
            workspace
        })
        .collect()
}

fn active_panes(pane_count: usize) -> Vec<Workspace> {
    let history = history();
    let mut workspace = Workspace::test_new("bench");
    let root_pane = workspace.tabs[0].root_pane;
    workspace.insert_test_runtime(root_pane, runtime(&history));
    let mut pane_ids = vec![root_pane];

    for index in 1..pane_count {
        let target = pane_ids[(index - 1) / 2];
        workspace.tabs[0].layout.focus_pane(target);
        let direction = if index % 2 == 0 {
            Direction::Vertical
        } else {
            Direction::Horizontal
        };
        let pane_id = workspace.test_split(direction);
        workspace.insert_test_runtime(pane_id, runtime(&history));
        pane_ids.push(pane_id);
    }

    vec![workspace]
}

fn summarize(mut samples: Vec<Duration>) -> StageStats {
    samples.sort_unstable();
    StageStats {
        median_us: samples[SAMPLE_COUNT / 2].as_micros(),
        p95_us: samples[(SAMPLE_COUNT - 1) * 95 / 100].as_micros(),
        max_us: samples[SAMPLE_COUNT - 1].as_micros(),
    }
}

fn profile(build: fn(usize) -> Vec<Workspace>, count: usize) -> PipelineStats {
    profile_pipeline(RenderPipeline::new(build(count)))
}

fn profile_pipeline(mut pipeline: RenderPipeline) -> PipelineStats {
    for _ in 0..WARMUP_COUNT {
        black_box(pipeline.render_once());
    }

    let mut server = Vec::with_capacity(SAMPLE_COUNT);
    let mut client = Vec::with_capacity(SAMPLE_COUNT);
    let mut total = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let (server_elapsed, client_elapsed) = pipeline.render_once();
        server.push(server_elapsed);
        client.push(client_elapsed);
        total.push(server_elapsed + client_elapsed);
    }

    PipelineStats {
        server: summarize(server),
        client: summarize(client),
        total: summarize(total),
    }
}

fn print_stage(
    label: &str,
    rows: &[(usize, PipelineStats)],
    stage: fn(&PipelineStats) -> StageStats,
) {
    let baseline = stage(&rows[0].1);
    println!("  {label}");
    println!("       count  median_us  p95_us  max_us  median_vs_1x  p95_vs_1x");
    for (count, pipeline) in rows {
        let stats = stage(pipeline);
        println!(
            "  {count:>10}  {:>9}  {:>6}  {:>6}  {:>12.2}  {:>9.2}",
            stats.median_us,
            stats.p95_us,
            stats.max_us,
            stats.median_us as f64 / baseline.median_us.max(1) as f64,
            stats.p95_us as f64 / baseline.p95_us.max(1) as f64,
        );
    }
}

fn print_profiles(label: &str, build: fn(usize) -> Vec<Workspace>) {
    let rows = CARDINALITIES.map(|count| (count, profile(build, count)));
    println!("{label}");
    print_stage("server pane surface", &rows, |stats| stats.server);
    print_stage("client shell composition", &rows, |stats| stats.client);
    print_stage("combined pipeline", &rows, |stats| stats.total);
}

fn profile_snapshot_encoding(
    build: fn(usize) -> Vec<Workspace>,
    count: usize,
    client_count: usize,
) -> StageStats {
    let pipeline = RenderPipeline::new(build(count));
    let run = || {
        let started = Instant::now();
        let template = crate::server::rendering::snapshot::snapshot(
            &pipeline.app,
            "bench-boot",
            1,
            None,
            None,
        );
        for client_index in 0..client_count {
            let mut snapshot = template.clone();
            snapshot.revision = client_index as u64 + 1;
            let message = crate::protocol::wire::handshake::snapshot_message(&snapshot)
                .expect("benchmark snapshot should serialize");
            black_box(
                bincode::serde::encode_to_vec(message, bincode::config::standard())
                    .expect("benchmark snapshot message should frame"),
            );
        }
        started.elapsed()
    };
    for _ in 0..WARMUP_COUNT {
        black_box(run());
    }
    summarize((0..SAMPLE_COUNT).map(|_| run()).collect())
}

fn print_snapshot_encoding_profiles(label: &str, build: fn(usize) -> Vec<Workspace>) {
    println!("{label} snapshot projection + JSON framing");
    println!("       panes  clients  median_us  p95_us  max_us");
    for count in CARDINALITIES {
        for client_count in CLIENT_CARDINALITIES {
            let stats = profile_snapshot_encoding(build, count, client_count);
            println!(
                "  {count:>10}  {client_count:>7}  {:>9}  {:>6}  {:>6}",
                stats.median_us, stats.p95_us, stats.max_us
            );
        }
    }
}

fn print_token_rule_profiles() {
    let rules = std::iter::repeat_n(
        "{ contains = 'NO-MATCH', ignore_case = true, bold = true }",
        15,
    )
    .chain(std::iter::once("{ starts_with = 'bench', bold = true }"))
    .collect::<Vec<_>>()
    .join(",");
    for (label, build) in [
        ("background", workspaces as fn(usize) -> Vec<Workspace>),
        ("active", active_panes),
    ] {
        for conditional in [false, true] {
            let config: Config = toml::from_str(&format!(
                "[ui.sidebar.agents]\nrows = [[{{ token = 'workspace', rules = [{}] }}]]\n[ui.sidebar.spaces]\nrows = [[{{ token = 'workspace', rules = [{}] }}]]",
                if conditional { &rules } else { "" }, if conditional { &rules } else { "" },
            )).unwrap();
            let rows = [1, 15].map(|count| {
                let mut pipeline = RenderPipeline::with_config(build(count), &config);
                pipeline.app.state.ensure_test_terminals();
                for terminal in pipeline.app.state.terminals.values_mut() {
                    terminal.detected_agent = Some(crate::agents::AgentKind::Pi);
                }
                pipeline.client.set_snapshot(Box::new(
                    crate::server::rendering::snapshot::snapshot(
                        &pipeline.app,
                        "bench-boot",
                        1,
                        None,
                        None,
                    ),
                ));
                (count, profile_pipeline(pipeline))
            });
            println!(
                "token rules {label}: populated agents, rules_per_token={}",
                if conditional { 16 } else { 0 }
            );
            print_stage("client shell composition", &rows, |stats| stats.client);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "manual client-rendered pipeline scaling profile"]
async fn render_scale_profile() {
    print_profiles("background workspaces (one pane each)", workspaces);
    print_snapshot_encoding_profiles("background workspaces", workspaces);
    print_profiles("active panes (one workspace)", active_panes);
    print_snapshot_encoding_profiles("active panes", active_panes);
    print_token_rule_profiles();
}

#[test]
fn direct_placement_and_unicode_fragment_keep_distinct_host_identities() {
    use crate::client::host_terminal::kitty::{
        encode_graphics_update_incremental, HostGraphicsCache,
    };
    use crate::protocol::kitty::apc::encode_kitty_data;
    use crate::protocol::kitty::placement::{HostPlacement, HostSourceKey};
    use crate::terminal::vt::Terminal;
    use crate::utils::ids::PaneId;

    let mut terminal = Terminal::new(10, 5, 0).unwrap();
    terminal.enable_kitty_graphics().unwrap();
    terminal.resize(10, 5, 10, 10).unwrap();
    let mut upload = Vec::new();
    encode_kitty_data(
        &mut upload,
        "a=T,U=1,t=d,f=32,s=20,v=10,i=42,c=2,r=1,q=2",
        &vec![255; 20 * 10 * 4],
    );
    terminal.write(&upload);
    terminal.write(
        "\x1b[1;1H\x1b[38;5;42m\u{10eeee}\u{0305}\u{0305}\u{10eeee}\u{0305}\u{030d}\x1b[0m"
            .as_bytes(),
    );
    let initial = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(initial.len(), 1);
    let fragment_id = initial[0].placement_id;

    // Direct placement IDs belong to the child application; a valid chosen
    // ID must not overwrite the separately displayed Unicode fragment.
    terminal
        .write(format!("\x1b[3;5H\x1b_Ga=p,i=42,p={fragment_id},c=2,r=1,C=1,q=2\x1b\\").as_bytes());
    let placements = terminal
        .kitty_image_placements_with_data_filter(|_| true)
        .unwrap();
    assert_eq!(placements.len(), 2);
    let placements = placements
        .into_iter()
        .map(|image| HostPlacement {
            pane_id: PaneId::from_raw(1),
            host_image_id: None,
            area: Rect::new(0, 0, 10, 5),
            cell_size: HostCellSize {
                width_px: 10,
                height_px: 10,
            },
            source_key: HostSourceKey::Terminal {
                pane_id: PaneId::from_raw(1),
                image_id: image.image_id,
            },
            placement: image,
            scrollback_offset: 0,
        })
        .collect::<Vec<_>>();

    let mut host_terminal = Terminal::new(10, 5, 0).unwrap();
    host_terminal.enable_kitty_graphics().unwrap();
    host_terminal.resize(10, 5, 10, 10).unwrap();
    let mut cache = HostGraphicsCache::default();
    let mut incomplete = true;
    for _ in 0..4 {
        let encoded = encode_graphics_update_incremental(&mut cache, &placements, None, false);
        host_terminal.write(&encoded.bytes);
        incomplete = encoded.incomplete;
        if !incomplete {
            break;
        }
    }
    assert!(
        !incomplete,
        "two unchanged placements must finish rendering"
    );
    assert_eq!(
        host_terminal
            .kitty_image_placements_with_data_filter(|_| true)
            .unwrap()
            .len(),
        2,
        "the host terminal must display both placements"
    );
}
