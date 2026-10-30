use super::*;
use crate::messaging::{
    coordinator::{BusEvent, BusSnapshot},
    model::BusState,
    prefs::settings::BusSettings,
};
use std::sync::Mutex;

struct RecordingTransport(Arc<Mutex<Vec<ClientMessage>>>);

impl endpoint::EndpointTransport for RecordingTransport {
    fn send(&mut self, message: &ClientMessage) -> io::Result<()> {
        self.0.lock().unwrap().push(message.clone());
        Ok(())
    }
}

fn driver(cols: u16) -> (ClientLoopDriver, Arc<Mutex<Vec<ClientMessage>>>) {
    let mut rooms = BusState::default();
    rooms.create_room("width").unwrap();
    let mut shell = shell::ClientShellState::new(shell::ClientShellConfig::from_config(
        &crate::utils::config::Config::default(),
    ));
    shell.bus = Some(crate::client::rooms::BusUi::new(Arc::new(BusSnapshot {
        state: rooms,
        revision: 0,
        last_command_id: 0,
        error: None,
    })));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let state = ClientState {
        output: crate::client::host_terminal::output::TerminalOutput::start().unwrap(),
        mouse_capture_active: false,
        endpoint_mouse_capture_requested: false,
        endpoint_sgr_pixels_requested: false,
        host_palette_query_pending: Arc::default(),
        host_palette_query_progress: Arc::default(),
        shell_mouse_capture_preference: false,
        pane_keyboard_report_all: false,
        keyboard_report_all_active: false,
        reported_size: (cols, 40),
        reported_cell_size: (8, 16),
        sound_config: crate::utils::config::SoundConfig::default(),
        kitty_graphics_enabled: false,
        pixel_geometry_enabled: false,
        pixel_geometry_exact: true,
        redraw_on_focus_gained: false,
        repaint_pending: false,
        draw_host_cursor: false,
        detached_process_children: Vec::new(),
        shell: Some(shell),
    };
    (
        ClientLoopDriver {
            write_stream: endpoint::ServerConnection::new(RecordingTransport(sent.clone())),
            endpoint_commands: endpoint_commands::EndpointCommands::default(),
            host_sgr_pixels_active: Arc::default(),
            #[cfg(unix)]
            reported_cell_size: Arc::default(),
            host_mouse_capture_active: Arc::default(),
            state,
            #[cfg(unix)]
            will_query_host_cell_size: false,
            deferred_event: None,
            ignore_disconnects: 0,
        },
        sent,
    )
}

fn receive_width(driver: &mut ClientLoopDriver, width: u16) {
    driver
        .apply_shell_input(|shell| {
            shell
                .bus
                .as_mut()
                .unwrap()
                .receive_event(BusEvent::SettingsChanged(BusSettings {
                    sidebar_width: width,
                    ..Default::default()
                }));
            shell::ClientShellInput::default()
        })
        .unwrap();
}

#[test]
fn sidebar_width_change_sends_the_new_surface_geometry_to_the_server() {
    let (mut driver, sent) = driver(120);
    receive_width(&mut driver, 40);
    let messages = sent.lock().unwrap().clone();
    assert!(
        matches!(
            messages.as_slice(),
            [ClientMessage::ClientShellResize {
                surface_size,
                cell_width_px: 8,
                cell_height_px: 16,
                pixel_mouse: true,
            }] if surface_size.cols == 80 && surface_size.rows == 40
        ),
        "the live width change must resize the server's panes: {messages:?}"
    );
    receive_width(&mut driver, 40);
    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "an unchanged width sends no resize"
    );
    receive_width(&mut driver, 20);
    assert!(matches!(sent.lock().unwrap().last(),
        Some(ClientMessage::ClientShellResize { surface_size, .. }) if surface_size.cols == 100));
}

#[test]
fn sidebar_width_change_clamped_to_the_same_geometry_sends_no_resize() {
    let (mut driver, sent) = driver(80);
    receive_width(&mut driver, 60);
    assert!(
        sent.lock().unwrap().is_empty(),
        "both widths render at 26 columns"
    );
}

#[cfg(unix)]
#[path = "slow_terminal_test.rs"]
mod slow_terminal;

#[cfg(unix)]
#[test]
fn a_restart_stop_shutdown_is_not_a_final_client_exit() {
    use interprocess::local_socket::traits::Listener as _;

    let _lock = crate::utils::config::test_config_env_lock().lock().unwrap();
    crate::utils::paths::clear_explicit_session_for_test();
    let root = crate::utils::test_temp::unique_temp_path("restart-shutdown");
    std::fs::create_dir_all(&root).unwrap();
    let socket = root.join("client.sock");
    let sha = "ab".repeat(20);
    crate::utils::restart_record::write_json(
        &crate::utils::restart_record::build_path(&root),
        &serde_json::json!({ "sha": sha, "binary": "/tmp/bus" }),
    )
    .unwrap();
    crate::utils::restart_record::write_json(
        &crate::utils::restart_record::marker_path(&root),
        &crate::utils::restart_record::RestartMarker {
            binary: "/tmp/bus".into(),
            session_id: None,
            explicit_data_dir: true,
            data_dir: root.display().to_string(),
            dev: true,
            yolo: false,
        },
    )
    .unwrap();
    let saved = [
        ("BUS_DATA_DIR", std::env::var_os("BUS_DATA_DIR")),
        (
            crate::utils::env::SOCKET_PATH_ENV_VAR,
            std::env::var_os(crate::utils::env::SOCKET_PATH_ENV_VAR),
        ),
        (
            crate::utils::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR,
            std::env::var_os(crate::utils::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR),
        ),
    ];
    std::env::set_var("BUS_DATA_DIR", &root);
    std::env::remove_var(crate::utils::env::SOCKET_PATH_ENV_VAR);
    std::env::set_var(
        crate::utils::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR,
        &socket,
    );
    let listener = crate::platform::ipc::bind_local_listener(&socket).unwrap();
    let accept = std::thread::spawn(move || {
        for _ in 0..6 {
            if listener.accept().is_err() {
                break;
            }
        }
    });
    let (mut driver, _) = driver(80);
    let (event_tx, mut event_rx) = crate::utils::perf::queue::channel(8);
    let error = ClientError::ServerShutdown {
        reason: Some("server is shutting down".into()),
    };
    let result = driver.recover_restart(&event_tx, &mut event_rx, &error);
    for _ in 0..6 {
        let _ = crate::platform::ipc::connect_local_stream(&socket);
    }
    let _ = accept.join();
    for (key, value) in saved {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        result.is_some(),
        "a restart stop must not be a final client exit"
    );
}
