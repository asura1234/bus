#[cfg(unix)]
use super::finish_terminal_input;
use super::{
    client_socket_path, do_handshake, handshake, init_logging, initial_terminal_geometry,
    run_client_loop, setup_terminal, shell, stop_server_after_quit, ClientError,
};
use crate::ipc::LocalStream;
use std::{
    io::{self, Write as _},
    sync::{
        atomic::{AtomicBool, AtomicU16, Ordering},
        Arc,
    },
    time::Duration,
};
use tracing::{info, warn};

/// Runs the thin client and enters the main event loop.
pub fn run_client(
    logging_options: crate::utils::logging::LoggingOptions,
    config_override: Option<fn(&mut crate::config::Config)>,
    stop_server: fn() -> Result<(), String>,
) -> io::Result<()> {
    run_client_with_mode(
        "connecting to server",
        logging_options,
        config_override,
        stop_server,
    )
}

pub(super) struct ClientLoopConfig {
    pub(super) sound_config: crate::config::SoundConfig,
    pub(super) mouse_scroll_lines: usize,
    pub(super) redraw_on_focus_gained: bool,
    pub(super) host_cursor: crate::config::HostCursorModeConfig,
    pub(super) kitty_graphics_enabled: bool,
    pub(super) pixel_geometry_enabled: bool,
    pub(super) pixel_geometry_fallback: bool,
    pub(super) mouse_capture_active: bool,
    pub(super) shell_config: Option<shell::ClientShellConfig>,
}

#[derive(Clone, Default)]
pub(super) struct ClientInputLifecycle {
    pub(super) reader_should_quit: Arc<AtomicBool>,
    pub(super) host_palette_query_pending: Arc<AtomicBool>,
    pub(super) host_palette_query_progress: Arc<AtomicU16>,
}

fn run_client_with_mode(
    log_message: &'static str,
    logging_options: crate::utils::logging::LoggingOptions,
    config_override: Option<fn(&mut crate::config::Config)>,
    stop_server: fn() -> Result<(), String>,
) -> io::Result<()> {
    init_logging(&logging_options);

    let mut loaded_config = crate::config::Config::load();
    if let Some(apply) = config_override {
        apply(&mut loaded_config.config);
    }
    crate::terminal_modes::clear_host_mouse_reporting(&mut io::stdout())?;
    let socket_path = client_socket_path();
    let loop_config = client_loop_config(loaded_config.config, &loaded_config.diagnostics);
    let mouse_capture = loop_config.mouse_capture_active;

    crate::utils::logging::startup("client", logging_options.dev);
    info!(path = %socket_path.display(), "{log_message}");

    let InitialClientConnection {
        cols,
        rows,
        cell_width_px,
        cell_height_px,
        exact_cell_size,
        initial,
    } = initial_client_connection(&socket_path, &loop_config)?;

    let terminal_guard = setup_terminal(mouse_capture).map_err(|err| {
        eprintln!("herdr: failed to set up terminal: {err}");
        err
    })?;

    // Install a panic hook so the foreground client always restores its terminal.
    let panic_restore = terminal_guard.panic_restore();
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        panic_restore();
        original_hook(info);
    }));

    // Create the tokio runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(io::Error::other)?;

    let should_quit = Arc::new(AtomicBool::new(false));
    let input_lifecycle = ClientInputLifecycle::default();

    install_client_termination_handler(&should_quit);

    let result = rt.block_on(async {
        run_client_loop(
            initial,
            cols,
            rows,
            cell_width_px,
            cell_height_px,
            exact_cell_size,
            should_quit,
            input_lifecycle.clone(),
            loop_config,
        )
        .await
    });

    // Terminal palette replies are stdin, so let the reader drain them before restoring echo.
    #[cfg(unix)]
    let _ = finish_terminal_input(
        &input_lifecycle.host_palette_query_pending,
        &input_lifecycle.host_palette_query_progress,
        &input_lifecycle.reader_should_quit,
        Duration::from_millis(250),
        Duration::from_secs(2),
    );
    #[cfg(not(unix))]
    input_lifecycle
        .reader_should_quit
        .store(true, Ordering::Release);

    // Restore the terminal before printing any final status message.
    let terminal_restore_failed = terminal_guard.restore().is_err();
    let stopped = stop_server_after_quit(|| {
        eprintln!("Stopping Bus and its agents...");
        stop_server()
    });
    if let Err(error) = &stopped {
        let _ = writeln!(io::stderr(), "bus: could not stop the server: {error}");
    }

    finish_client_run(result, rt, terminal_restore_failed, stopped)
}

struct InitialClientConnection {
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
    exact_cell_size: bool,
    initial: Option<(LocalStream, handshake::HandshakeResult)>,
}

fn initial_client_connection(
    socket_path: &std::path::Path,
    loop_config: &ClientLoopConfig,
) -> io::Result<InitialClientConnection> {
    let initial_stream = match crate::ipc::connect_local_stream(socket_path) {
        Ok(stream) => Some(stream),
        Err(error) => {
            return Err(io::Error::other(
                ClientError::ConnectionFailed(error).to_string(),
            ));
        }
    };

    // Get the terminal geometry before handshake (before raw mode).
    let (cols, rows, cell_width_px, cell_height_px, exact_cell_size) = initial_terminal_geometry(
        loop_config.pixel_geometry_enabled,
        loop_config.kitty_graphics_enabled,
    )?;

    let shell_surface_size = loop_config
        .shell_config
        .as_ref()
        .expect("client shell")
        .initial_surface_size(cols, rows);
    // Healthy Local connects directly; only an actual failure enters background recovery.
    let initial: io::Result<Option<(LocalStream, handshake::HandshakeResult)>> = initial_stream
        .map(|mut stream| {
            let handshake = do_handshake(
                &mut stream,
                cell_width_px,
                cell_height_px,
                exact_cell_size,
                shell_surface_size,
                false,
                loop_config.mouse_capture_active,
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
            Ok((stream, handshake))
        })
        .transpose();
    Ok(InitialClientConnection {
        cols,
        rows,
        cell_width_px,
        cell_height_px,
        exact_cell_size,
        initial: initial?,
    })
}

fn install_client_termination_handler(should_quit: &Arc<AtomicBool>) {
    // ctrlc's "termination" feature also catches SIGTERM/SIGHUP so direct
    // termination signals still run the quit path and TerminalGuard::Drop.
    let quit_flag = should_quit.clone();
    if let Err(err) = ctrlc::set_handler(move || {
        quit_flag.store(true, Ordering::Release);
    }) {
        warn!(%err, "failed to install termination handler; terminal restore relies on TerminalGuard::Drop and the panic hook");
    }
    // Ctrl+C belongs to the agents, so a SIGINT (for example from a terminal
    // that still delivers one in raw mode) must not quit the client. This runs
    // after ctrlc so it replaces only ctrlc's SIGINT hook; SIGTERM and SIGHUP
    // still take the quit path above.
    crate::platform::disregard_interrupt_signal();
}

fn finish_client_run(
    result: Result<(), ClientError>,
    rt: tokio::runtime::Runtime,
    terminal_restore_failed: bool,
    stopped: Result<(), String>,
) -> io::Result<()> {
    if let Err(err) = result {
        let _ = writeln!(io::stderr(), "herdr: {err}");
        rt.shutdown_timeout(Duration::from_millis(100));
        crate::logging::shutdown("client");

        let detached = matches!(
            &err,
            ClientError::ServerShutdown {
                reason: Some(reason)
            } if reason == "detached"
        );
        let connection_lost_during_terminal_hangup =
            terminal_restore_failed && matches!(&err, ClientError::ConnectionLost(_));
        if detached || connection_lost_during_terminal_hangup {
            return Ok(());
        }

        std::process::exit(1);
    }

    rt.shutdown_timeout(Duration::from_millis(100));
    crate::logging::shutdown("client");
    stopped.map_err(io::Error::other)
}

fn client_loop_config(config: crate::config::Config, diagnostics: &[String]) -> ClientLoopConfig {
    let startup_config_diagnostic = crate::config::config_diagnostic_summary(diagnostics);
    let shell_config = Some(
        shell::ClientShellConfig::from_config(&config)
            .with_startup_config_diagnostic(startup_config_diagnostic),
    );
    let mouse_capture = config.ui.mouse_capture;
    let mouse_scroll_lines = config.ui.mouse_scroll_lines();
    let redraw_on_focus_gained = config.ui.redraw_on_focus_gained;
    let host_cursor = config.ui.host_cursor;
    let kitty_graphics_enabled = config.kitty_graphics_enabled();
    let pixel_geometry_enabled = kitty_graphics_enabled;
    ClientLoopConfig {
        sound_config: config.ui.sound,
        mouse_scroll_lines,
        redraw_on_focus_gained,
        host_cursor,
        kitty_graphics_enabled,
        pixel_geometry_enabled,
        pixel_geometry_fallback: kitty_graphics_enabled,
        mouse_capture_active: mouse_capture,
        shell_config,
    }
}
