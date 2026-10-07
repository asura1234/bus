//! Thin client mode — connects to the server's client socket.
//!
//! The client:
//! - Connects to `herdr-client.sock`, sends the shell hello with this build's version
//! - Sets up the real terminal (raw mode, mouse capture, keyboard enhancements)
//! - Receives Frame messages and blits them to the terminal (diff against last frame)
//! - Reads stdin events (keystrokes, mouse, paste) and sends them as ClientMessage::Input
//! - Detects terminal resize and sends ClientMessage::Resize
//! - Restores terminal on exit (normal or error)
//! - Handles ServerShutdown gracefully (clean exit, informative message to stderr)
//! - Handles server unreachable (clear error screen, not blank/hang)
//! - Forwards OSC 52 clipboard writes from server to its own stdout
//! - Displays sound/toast notifications forwarded from server

mod clipboard_forwarding;
mod config_reload;
pub(crate) mod endpoint;
mod endpoint_commands;
mod errors;
mod events;
mod frame_output;
mod handshake;
mod input;
mod loop_config;
mod notifications;
mod shell;
mod shell_runtime;
mod startup;
mod state;
mod terminal_geometry;
mod terminal_setup;
mod timer;
mod transport;

#[cfg(test)]
use clipboard_forwarding::decode_clipboard_payload;
use clipboard_forwarding::forward_clipboard;
#[cfg(test)]
use config_reload::reload_local_client_config;
use config_reload::{apply_reload, init_logging};
use events::ClientLoopEvent;
use loop_config::ClientLoopConfig;
use shell_runtime::*;
use state::ClientState;
use transport::*;

#[cfg(test)]
pub(crate) use shell::{ClientShellConfig, ClientShellState};
pub use startup::run_client;

#[cfg(not(windows))]
use terminal_geometry::query_host_terminal_appearance;
#[cfg(test)]
use terminal_geometry::{
    cell_size_fallback, current_terminal_geometry_with, ioctl_cell_size, pack_cell_size,
    resize_report_required, should_query_host_cell_size, write_host_cell_size_query,
    write_host_terminal_appearance_query, write_host_terminal_theme_query,
};
use terminal_geometry::{
    host_cell_size_query_required, initial_terminal_geometry, query_host_cell_size,
    query_host_terminal_theme, resize_poll_loop, should_query_host_terminal_theme,
};
#[cfg(unix)]
use terminal_geometry::{reported_cell_size_from_events, store_reported_cell_size};
#[cfg(unix)]
use terminal_setup::finish_terminal_input;
use terminal_setup::{
    effective_sgr_pixel_mouse, set_mouse_capture, setup_terminal, should_draw_host_cursor,
};
#[cfg(windows)]
use terminal_setup::{
    enable_windows_virtual_terminal_input, is_ssh_session, windows_vti_input_backend_enabled,
};
#[cfg(test)]
use terminal_setup::{
    should_enable_host_color_scheme_reports, windows_virtual_terminal_input_mode,
    write_host_color_scheme_report_mode, write_terminal_restore_postlude,
};

pub use errors::ClientError;
#[cfg(test)]
use frame_output::{clear_received_kitty_graphics, kitty_graphics_image_ids};
use frame_output::{
    contains_kitty_graphics_bytes, record_received_kitty_graphics,
    write_encoded_frame_with_graphics,
};
#[cfg(test)]
use handshake::direct_graphics_profile_values;
use handshake::do_handshake;
use notifications::{forward_terminal_bells, handle_notify};
#[cfg(test)]
use notifications::{handle_notify_with_notifiers, sound_from_notify_message};

use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use interprocess::local_socket::traits::Stream as _;
use interprocess::TryClone as _;
use tracing::{debug, info, warn};

use crate::ipc::LocalStream;
use crate::protocol::render_ansi;
#[cfg(test)]
use crate::protocol::NotifyKind;
use crate::protocol::{self, ClientMessage, FrameData, ServerMessage, MAX_GRAPHICS_FRAME_SIZE};
use crate::server::socket_paths::client_socket_path;

#[derive(Clone, Default)]
struct ClientInputLifecycle {
    reader_should_quit: Arc<AtomicBool>,
    host_palette_query_pending: Arc<AtomicBool>,
    host_palette_query_progress: Arc<AtomicU16>,
}

fn run_client_with_mode(log_message: &'static str) -> io::Result<()> {
    init_logging();

    let loaded_config = crate::config::Config::load();
    crate::terminal_modes::clear_host_mouse_reporting(&mut io::stdout())?;
    let socket_path = client_socket_path();
    let startup_config_diagnostic =
        crate::config::config_diagnostic_summary(&loaded_config.diagnostics);
    let shell_config = Some(
        shell::ClientShellConfig::from_config(&loaded_config.config)
            .with_startup_config_diagnostic(startup_config_diagnostic),
    );
    let mouse_capture = loaded_config.config.ui.mouse_capture;
    let mouse_scroll_lines = loaded_config.config.ui.mouse_scroll_lines();
    let redraw_on_focus_gained = loaded_config.config.ui.redraw_on_focus_gained;
    let host_cursor = loaded_config.config.ui.host_cursor;
    let kitty_graphics_enabled = loaded_config.config.kitty_graphics_enabled();
    let pixel_geometry_enabled = kitty_graphics_enabled;
    let loop_config = ClientLoopConfig {
        sound_config: loaded_config.config.ui.sound,
        mouse_scroll_lines,
        redraw_on_focus_gained,
        host_cursor,
        kitty_graphics_enabled,
        pixel_geometry_enabled,
        pixel_geometry_fallback: kitty_graphics_enabled,
        mouse_capture_active: mouse_capture,
        shell_config,
    };

    crate::logging::startup("client");
    info!(path = %socket_path.display(), "{log_message}");

    let initial_stream = match crate::ipc::connect_local_stream(&socket_path) {
        Ok(stream) => Some(stream),
        Err(error) => {
            return Err(io::Error::other(
                ClientError::ConnectionFailed(error).to_string(),
            ));
        }
    };

    // Get the terminal geometry before handshake (before raw mode).
    let (cols, rows, cell_width_px, cell_height_px, exact_cell_size) =
        initial_terminal_geometry(pixel_geometry_enabled, kitty_graphics_enabled)?;

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
    let initial = initial?;

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
    Ok(())
}

/// The main client event loop.
///
/// Uses a threaded architecture:
/// - stdin reader thread → sends raw input bytes to main loop
/// - resize poller thread → sends resize events to main loop
/// - server reader thread → reads ServerMessages and sends to main loop
/// - main loop: coordinates input, output, and server communication
async fn run_client_loop(
    initial: Option<(LocalStream, handshake::HandshakeResult)>,
    cols: u16,
    rows: u16,
    initial_cell_width_px: u32,
    initial_cell_height_px: u32,
    initial_pixel_geometry_exact: bool,
    should_quit: Arc<AtomicBool>,
    input_lifecycle: ClientInputLifecycle,
    config: ClientLoopConfig,
) -> Result<(), ClientError> {
    let _ = config.mouse_scroll_lines;
    let draw_host_cursor = should_draw_host_cursor(config.host_cursor);
    let mut state = ClientState {
        blit_encoder: render_ansi::BlitEncoder::new(),
        mouse_capture_active: config.mouse_capture_active,
        endpoint_mouse_capture_requested: false,
        endpoint_sgr_pixels_requested: false,
        host_palette_query_pending: input_lifecycle.host_palette_query_pending,
        host_palette_query_progress: input_lifecycle.host_palette_query_progress,
        shell_mouse_capture_preference: config.mouse_capture_active,
        pane_keyboard_report_all: false,
        keyboard_report_all_active: false,
        reported_size: (cols, rows),
        reported_cell_size: (initial_cell_width_px, initial_cell_height_px),
        sound_config: config.sound_config,
        kitty_graphics_enabled: config.kitty_graphics_enabled,
        pixel_geometry_enabled: config.pixel_geometry_enabled,
        pixel_geometry_exact: initial_pixel_geometry_exact,
        redraw_on_focus_gained: config.redraw_on_focus_gained,
        repaint_pending: false,
        draw_host_cursor,
        detached_process_children: Vec::new(),
        shell: config.shell_config.map(shell::ClientShellState::new),
    };
    if let Some(shell) = state.shell.as_mut() {
        shell
            .start_bus(&state.sound_config)
            .map_err(|error| ClientError::ConnectionFailed(io::Error::other(error)))?;
        shell.set_graphics_cell_size(initial_cell_width_px, initial_cell_height_px);
        shell.set_bus_kitty_graphics(state.kitty_graphics_enabled);
    }
    let host_mouse_capture_active = Arc::new(AtomicBool::new(state.mouse_capture_active));
    // Cell size reported by the host terminal, packed as width<<32 | height.
    // Zero means the host has not reported one.
    let reported_cell_size = Arc::new(AtomicU64::new(0));
    let host_sgr_pixels_active = Arc::new(AtomicBool::new(false));

    // Channel for events from the resize and server reader threads.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ClientLoopEvent>(256);
    // Keep Windows console draining independent of server-frame backpressure.
    #[cfg(windows)]
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<ClientLoopEvent>(256);
    #[cfg(unix)]
    let stdin_tx = event_tx.clone();

    let mut endpoint_commands = endpoint_commands::EndpointCommands::default();

    // Spawn the stdin reader thread.
    let will_query_host_terminal_theme = should_query_host_terminal_theme();
    // Terminals behind ConPTY report no pixel size through the ioctl, so ask the
    // host terminal directly instead of falling back to an assumed cell size.
    let will_query_host_cell_size = host_cell_size_query_required(state.kitty_graphics_enabled);
    let stdin_quit = input_lifecycle.reader_should_quit;
    let stdin_mouse_capture_active = host_mouse_capture_active.clone();
    let stdin_sgr_pixels_active = host_sgr_pixels_active.clone();
    let stdin_host_palette_query_pending = state.host_palette_query_pending.clone();
    let stdin_host_palette_query_progress = state.host_palette_query_progress.clone();
    std::thread::spawn(move || {
        input::stdin_reader_loop(
            stdin_tx,
            &stdin_quit,
            will_query_host_terminal_theme,
            will_query_host_cell_size,
            stdin_host_palette_query_pending,
            stdin_host_palette_query_progress,
            stdin_mouse_capture_active,
            stdin_sgr_pixels_active,
        );
    });

    if will_query_host_terminal_theme {
        query_host_terminal_theme(
            &state.host_palette_query_pending,
            &state.host_palette_query_progress,
        );
        #[cfg(not(windows))]
        if state.shell.is_some() {
            query_host_terminal_appearance();
        }
    }

    if will_query_host_cell_size {
        query_host_cell_size();
    }

    // Spawn the resize poller thread.
    let resize_quit = should_quit.clone();
    let resize_tx = event_tx.clone();
    let resize_cell_size = reported_cell_size.clone();
    let pixel_geometry_enabled = state.pixel_geometry_enabled;
    let pixel_geometry_fallback = config.pixel_geometry_fallback;
    std::thread::spawn(move || {
        resize_poll_loop(
            resize_tx,
            cols,
            rows,
            initial_cell_width_px,
            initial_cell_height_px,
            initial_pixel_geometry_exact,
            pixel_geometry_enabled,
            pixel_geometry_fallback,
            &resize_cell_size,
            &resize_quit,
        );
    });

    let (stream, _handshake) = initial.expect("connected client");
    let max_frame_size = if state.kitty_graphics_enabled {
        MAX_GRAPHICS_FRAME_SIZE
    } else {
        crate::protocol::MAX_FRAME_SIZE
    };
    let transport = start_endpoint_transport(stream, (), &event_tx, max_frame_size)?;
    let mut write_stream = endpoint::ServerConnection::new(transport);
    if state.shell.is_some() {
        write_stream
            .send(&ClientMessage::ClientShellFocus { focused: true })
            .map_err(ClientError::ConnectionLost)?;
    }
    // Main event loop.
    let mut client_timer = timer::ClientLoopTimer::new();
    #[cfg(windows)]
    let mut stdin_open = true;
    while !should_quit.load(Ordering::Acquire) {
        let timer_delay = state
            .shell
            .as_ref()
            .map_or(Duration::from_millis(100), |shell| {
                shell.timer_delay(std::time::Instant::now())
            });
        let timer_deadline = client_timer.deadline(std::time::Instant::now(), timer_delay);
        #[cfg(windows)]
        let event = {
            tokio::select! {
                _ = tokio::time::sleep_until(timer_deadline.into()) => ClientLoopEvent::Timer,
                ev = stdin_rx.recv(), if stdin_open => match ev {
                    Some(event) => event,
                    None => {
                        stdin_open = false;
                        ClientLoopEvent::Timer
                    }
                },
                ev = event_rx.recv() => ev.unwrap_or(ClientLoopEvent::Timer),
            }
        };
        #[cfg(unix)]
        let event = {
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(timer_deadline.into()) => ClientLoopEvent::Timer,
                ev = event_rx.recv() => ev.unwrap_or(ClientLoopEvent::Timer),
            }
        };
        let now = std::time::Instant::now();

        match event {
            #[cfg(unix)]
            ClientLoopEvent::StdinInput(data) => {
                if state.shell.is_some() {
                    if will_query_host_cell_size {
                        let events = crate::raw_input::parse_raw_input_bytes_sync(&data);
                        if let Some((width_px, height_px)) = reported_cell_size_from_events(&events)
                        {
                            store_reported_cell_size(&reported_cell_size, width_px, height_px);
                        }
                    }
                    let (outcome, frame) = {
                        let shell = state.shell.as_mut().expect("checked shell mode");
                        let outcome = shell.handle_input_bytes(&data);
                        let frame = outcome
                            .repaint
                            .then(|| shell.compose(state.reported_size.0, state.reported_size.1))
                            .flatten();
                        (outcome, frame)
                    };
                    if finish_client_shell_input(
                        &mut state,
                        outcome,
                        frame,
                        &mut write_stream,
                        &mut endpoint_commands,
                    )? {
                        return Ok(());
                    }
                }
            }
            #[cfg(unix)]
            ClientLoopEvent::PixelMouse(data, geometry) => {
                if state.shell.is_some() {
                    let (outcome, frame) = {
                        let shell = state.shell.as_mut().expect("checked shell mode");
                        let outcome = shell.handle_pixel_mouse(&data, geometry);
                        let frame = outcome
                            .repaint
                            .then(|| shell.compose(state.reported_size.0, state.reported_size.1))
                            .flatten();
                        (outcome, frame)
                    };
                    if finish_client_shell_input(
                        &mut state,
                        outcome,
                        frame,
                        &mut write_stream,
                        &mut endpoint_commands,
                    )? {
                        return Ok(());
                    }
                }
            }
            #[cfg(windows)]
            ClientLoopEvent::StdinEvents(events) => {
                if state.shell.is_some() {
                    let (outcome, frame) = {
                        let shell = state.shell.as_mut().expect("checked shell mode");
                        let outcome = shell.handle_client_events(&events);
                        let frame = outcome
                            .repaint
                            .then(|| shell.compose(state.reported_size.0, state.reported_size.1))
                            .flatten();
                        (outcome, frame)
                    };
                    if finish_client_shell_input(
                        &mut state,
                        outcome,
                        frame,
                        &mut write_stream,
                        &mut endpoint_commands,
                    )? {
                        return Ok(());
                    }
                }
            }
            ClientLoopEvent::TerminalUnavailable(err) => {
                info!(err = %err, "client terminal unavailable; detaching");
                let _ = write_to_server(&mut write_stream, &ClientMessage::Detach);
                return Ok(());
            }
            ClientLoopEvent::Resize(
                new_cols,
                new_rows,
                cell_width_px,
                cell_height_px,
                pixel_geometry_exact,
            ) => {
                if !pixel_geometry_exact && host_sgr_pixels_active.load(Ordering::Acquire) {
                    set_mouse_capture(state.mouse_capture_active, false)
                        .map_err(ClientError::ConnectionFailed)?;
                    host_sgr_pixels_active.store(false, Ordering::Release);
                }
                state.reported_size = (new_cols, new_rows);
                state.reported_cell_size = (cell_width_px, cell_height_px);
                state.pixel_geometry_exact = pixel_geometry_exact;
                // Resizing invalidates both the host-side blit baseline and pane hit geometry.
                // The next frame clears the screen even at an unchanged size: the host
                // may show cells from the resize that no frame drew.
                state.blit_encoder.invalidate();
                state.request_repaint();
                if let Some(shell) = state.shell.as_mut() {
                    shell.set_graphics_cell_size(cell_width_px, cell_height_px);
                    shell.invalidate_pane_surface();
                }
                let msg = if let Some(shell) = &state.shell {
                    client_shell_resize_message(
                        shell,
                        new_cols,
                        new_rows,
                        cell_width_px,
                        cell_height_px,
                        pixel_geometry_exact,
                    )
                } else {
                    ClientMessage::Resize {
                        cols: new_cols,
                        rows: new_rows,
                        cell_width_px,
                        cell_height_px,
                        pixel_mouse: pixel_geometry_exact,
                    }
                };
                if let Err(e) = write_to_server(&mut write_stream, &msg) {
                    return Err(ClientError::ConnectionLost(e));
                }
                // The Bus view draws locally, so show it at the new size now
                // instead of keeping the old width until the next event.
                let frame = state
                    .shell
                    .as_mut()
                    .filter(|shell| shell.has_bus())
                    .and_then(|shell| shell.compose(new_cols, new_rows));
                if let Some(frame) = frame {
                    state.present_frame(frame);
                }
            }
            ClientLoopEvent::ServerMessage { message } => {
                if !write_stream.is_connected() {
                    continue;
                }
                match *message {
                    ServerMessage::ClientShellSnapshot(_) => {
                        let message = "server sent an unnegotiated binary endpoint snapshot";
                        return Err(ClientError::Protocol(protocol::FramingError::Io(
                            io::Error::new(io::ErrorKind::InvalidData, message),
                        )));
                    }
                    ServerMessage::PaneSurface(surface) => {
                        let composed = if let Some(shell) = &mut state.shell {
                            shell.set_pane_surface(surface);
                            shell.compose(state.reported_size.0, state.reported_size.1)
                        } else {
                            None
                        };
                        if let Some(frame) = composed {
                            state.present_frame(frame);
                        }
                    }
                    ServerMessage::PaneSurfacePatch(patch) => {
                        let patch_started = crate::render_prof::timer();
                        let apply_started = crate::render_prof::timer();
                        let outcome = state
                            .shell
                            .as_mut()
                            .map(|shell| shell.apply_pane_surface_patch(patch));
                        crate::render_prof::duration_since(
                            "client_surface_patch.apply",
                            apply_started,
                        );
                        let compose_fallback = match outcome {
                            Some(shell::ClientPaneSurfacePatchOutcome::Applied(Some(patch))) => {
                                match state.present_surface_patch(patch) {
                                    Ok(presented) => !presented,
                                    Err(error) => {
                                        warn!(%error, "failed to present retained pane surface patch");
                                        state.request_repaint();
                                        false
                                    }
                                }
                            }
                            Some(shell::ClientPaneSurfacePatchOutcome::Applied(None)) => true,
                            Some(shell::ClientPaneSurfacePatchOutcome::Rejected) | None => false,
                        };
                        if compose_fallback {
                            let composed = state.shell.as_mut().and_then(|shell| {
                                shell.compose(state.reported_size.0, state.reported_size.1)
                            });
                            if let Some(frame) = composed {
                                state.present_frame(frame);
                            }
                        }
                        crate::render_prof::duration_since(
                            "client_surface_patch.total",
                            patch_started,
                        );
                        crate::render_prof::flush_if_due();
                    }
                    ServerMessage::Terminal(frame) => {
                        if state.kitty_graphics_enabled
                            && contains_kitty_graphics_bytes(&frame.bytes)
                        {
                            record_received_kitty_graphics(&frame.bytes);
                        }
                        let mut stdout = io::stdout();
                        let _ = stdout.write_all(&frame.bytes);
                        let _ = stdout.flush();
                    }
                    ServerMessage::Graphics { bytes } => {
                        if state.kitty_graphics_enabled {
                            record_received_kitty_graphics(&bytes);
                            let mut stdout = io::stdout();
                            let _ = stdout.write_all(&bytes);
                            let _ = stdout.flush();
                        }
                    }
                    // Direct pane image transfers were removed; ignore them from older servers.
                    ServerMessage::GraphicsFile { .. }
                    | ServerMessage::GraphicsTransmissionRetired { .. } => {}
                    ServerMessage::TerminalBell { count } => {
                        let bus_shell = state.shell.as_ref().is_some_and(|shell| shell.has_bus());
                        if let Err(err) =
                            forward_terminal_bells(&mut io::stdout(), count, bus_shell)
                        {
                            warn!(err = %err, "failed to emit terminal bell");
                        }
                    }
                    ServerMessage::ServerShutdown { reason } => {
                        return Err(ClientError::ServerShutdown { reason });
                    }
                    ServerMessage::Notify {
                        kind,
                        message,
                        body,
                    } => {
                        if state.shell.is_none() {
                            handle_notify(kind, &message, body.as_deref(), &state.sound_config);
                        }
                    }
                    ServerMessage::SemanticNotification(_) => {}
                    ServerMessage::ClientShellError { message } => {
                        if let Some(shell) = state.shell.as_mut() {
                            if shell.receive_endpoint_error(message) {
                                let frame =
                                    shell.compose(state.reported_size.0, state.reported_size.1);
                                if let Some(frame) = frame {
                                    state.present_frame(frame);
                                }
                            }
                        }
                    }
                    ServerMessage::ClientShellEndpointResponseChunk {
                        boot_id,
                        request_id,
                        final_chunk,
                        data,
                    } => {
                        if request_id.starts_with("client-shell-surface:") {
                            continue;
                        }
                        let Some(completed) = endpoint_commands.receive_chunk(
                            &boot_id,
                            &request_id,
                            final_chunk,
                            data,
                        ) else {
                            continue;
                        };
                        let (repaint, actions) = state.shell.as_mut().map_or_else(
                            || (false, Vec::new()),
                            |shell| {
                                shell.handle_endpoint_result(
                                    &completed.boot_id,
                                    &completed.request_id,
                                    completed.result,
                                )
                            },
                        );
                        let (replay_mouse, dispatch_repaint) = dispatch_client_shell_actions(
                            actions,
                            &mut endpoint_commands,
                            &mut write_stream,
                            state.shell.as_mut(),
                            &mut state.detached_process_children,
                        )?;
                        let repaint = repaint || dispatch_repaint;
                        if replay_mouse.is_empty() {
                            if repaint {
                                if let Some(frame) = state.shell.as_mut().and_then(|shell| {
                                    shell.compose(state.reported_size.0, state.reported_size.1)
                                }) {
                                    state.present_frame(frame);
                                }
                            }
                        } else {
                            let (outcome, frame) = {
                                let shell = state.shell.as_mut().expect("shell endpoint response");
                                let mut outcome = shell.replay_mouse_events(replay_mouse);
                                outcome.repaint |= repaint;
                                let frame = outcome
                                    .repaint
                                    .then(|| {
                                        shell.compose(state.reported_size.0, state.reported_size.1)
                                    })
                                    .flatten();
                                (outcome, frame)
                            };
                            if finish_client_shell_input(
                                &mut state,
                                outcome,
                                frame,
                                &mut write_stream,
                                &mut endpoint_commands,
                            )? {
                                return Ok(());
                            }
                        }
                    }
                    ServerMessage::Clipboard { data } => {
                        if forward_clipboard(&data) {
                            let (width, height) = state.reported_size;
                            let frame = state.shell.as_mut().and_then(|shell| {
                                shell
                                    .show_copy_feedback(std::time::Instant::now())
                                    .then(|| shell.compose(width, height))
                                    .flatten()
                            });
                            if let Some(frame) = frame {
                                state.present_frame(frame);
                            }
                        }
                        let _ = io::stdout().flush();
                    }
                    ServerMessage::WindowTitle { title } => {
                        let _ = crate::terminal_effects::write_window_title(
                            &mut io::stdout(),
                            title.as_deref(),
                        );
                    }
                    ServerMessage::ReloadSoundConfig => {
                        apply_reload(&mut state, &mut write_stream)?
                    }
                    ServerMessage::MouseCapture {
                        enabled,
                        sgr_pixels,
                    } => {
                        state.endpoint_mouse_capture_requested = enabled;
                        state.endpoint_sgr_pixels_requested = sgr_pixels;
                        let next_sgr_pixels = effective_sgr_pixel_mouse(
                            enabled,
                            sgr_pixels,
                            state.pixel_geometry_exact,
                        );
                        let mouse_mode_changed = enabled != state.mouse_capture_active
                            || next_sgr_pixels != host_sgr_pixels_active.load(Ordering::Acquire);
                        if mouse_mode_changed {
                            #[cfg(windows)]
                            if enabled && windows_vti_input_backend_enabled() && is_ssh_session() {
                                let _ = enable_windows_virtual_terminal_input();
                            }
                            set_mouse_capture(enabled, next_sgr_pixels)
                                .map_err(ClientError::ConnectionFailed)?;
                            #[cfg(windows)]
                            if enabled && windows_vti_input_backend_enabled() && !is_ssh_session() {
                                let _ = enable_windows_virtual_terminal_input();
                            }
                        }
                        state.mouse_capture_active = enabled;
                        host_mouse_capture_active.store(enabled, Ordering::Release);
                        host_sgr_pixels_active.store(next_sgr_pixels, Ordering::Release);
                    }
                    ServerMessage::ClientShellKeyboardReportAll { enabled } => {
                        if state.shell.is_some() {
                            state.pane_keyboard_report_all = enabled;
                            sync_client_shell_keyboard_report_all(&mut state)?;
                        }
                    }
                    ServerMessage::EndpointControl { kind, data } => {
                        let snapshot = match endpoint::decode_endpoint_control(&kind, &data) {
                            Ok(endpoint::EndpointControlMessage::Ignored) => {
                                debug!(%kind, "ignoring unknown endpoint control message");
                                continue;
                            }
                            Ok(endpoint::EndpointControlMessage::Snapshot(snapshot)) => snapshot,
                            Err(message) => {
                                return Err(ClientError::Protocol(protocol::FramingError::Io(
                                    io::Error::new(io::ErrorKind::InvalidData, message),
                                )));
                            }
                        };
                        install_client_shell_snapshot(&mut state, snapshot, &mut write_stream)?;
                    }
                    ServerMessage::Welcome { .. } => {
                        debug!("received unexpected Welcome in main loop");
                    }
                }
            }
            ClientLoopEvent::ServerDisconnected => {
                write_stream.fail(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection was lost",
                ));
            }
            ClientLoopEvent::Timer => {
                client_timer.fired();
                state
                    .detached_process_children
                    .retain_mut(|child| child.try_wait().ok().flatten().is_none());
                if let Some(error) = write_stream.take_failure() {
                    warn!(error = %error, "server connection failed");
                    return Err(ClientError::ConnectionLost(error));
                }
                if state.shell.is_some() {
                    let expired = endpoint_commands.expire(now);
                    let (outcome, frame) = {
                        let shell = state.shell.as_mut().expect("checked shell mode");
                        let mut outcome = shell.tick_selection_autoscroll(now);
                        outcome.repaint |= shell.tick_bus();
                        outcome.detach |= shell.bus_exit_ready();
                        if let Some(expired) = expired {
                            let (repaint, actions) = shell.handle_endpoint_result(
                                &expired.boot_id,
                                &expired.request_id,
                                expired.result,
                            );
                            outcome.repaint |= repaint;
                            outcome.actions.extend(actions);
                        }
                        outcome.repaint |= shell.tick_copy_feedback(now);
                        let frame = outcome
                            .repaint
                            .then(|| shell.compose(state.reported_size.0, state.reported_size.1))
                            .flatten();
                        (outcome, frame)
                    };
                    if finish_client_shell_input(
                        &mut state,
                        outcome,
                        frame,
                        &mut write_stream,
                        &mut endpoint_commands,
                    )? {
                        return Ok(());
                    }
                }
            }
        }
    }

    // Clean exit (Ctrl+Q). Send Detach before closing.
    let detach = ClientMessage::Detach;
    let _ = write_to_server(&mut write_stream, &detach);
    let _ = io::stdout().flush();

    Ok(())
}

#[cfg(test)]
mod tests;
