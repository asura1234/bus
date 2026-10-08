//! Input, resize, timer, and connection event coordination.
#[cfg(not(windows))]
use super::query_host_terminal_appearance;
use super::{
    client_shell_resize_message, endpoint, endpoint_commands, finish_client_shell_input, handshake,
    host_cell_size_query_required, input, query_host_cell_size, query_host_terminal_theme,
    resize_poll_loop, server_messages, set_mouse_capture, shell, should_draw_host_cursor,
    should_query_host_terminal_theme, start_endpoint_transport, timer, write_to_server,
    ClientError, ClientInputLifecycle, ClientLoopConfig, ClientLoopEvent, ClientState,
};
#[cfg(unix)]
use super::{reported_cell_size_from_events, store_reported_cell_size};
use crate::{
    ipc::LocalStream,
    protocol::{render_ansi, ClientMessage, MAX_GRAPHICS_FRAME_SIZE},
};
use std::{
    io::{self, Write as _},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tracing::{info, warn};

/// The main client event loop.
///
/// Uses a threaded architecture:
/// - stdin reader thread → sends raw input bytes to main loop
/// - resize poller thread → sends resize events to main loop
/// - server reader thread → reads ServerMessages and sends to main loop
/// - main loop: coordinates input, output, and server communication
pub(super) async fn run_client_loop(
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
    let geometry = InitialClientGeometry {
        cols,
        rows,
        cell_width_px: initial_cell_width_px,
        cell_height_px: initial_cell_height_px,
        pixel_geometry_exact: initial_pixel_geometry_exact,
    };
    // Windows console draining remains independent of server-frame backpressure.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ClientLoopEvent>(256);
    #[cfg(windows)]
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<ClientLoopEvent>(256);
    #[cfg(unix)]
    let stdin_tx = event_tx.clone();
    let mut driver = start_client_loop(
        initial,
        geometry,
        &should_quit,
        input_lifecycle,
        config,
        &event_tx,
        stdin_tx,
    )?;
    let mut client_timer = timer::ClientLoopTimer::new();
    #[cfg(windows)]
    let mut stdin_open = true;
    while !should_quit.load(Ordering::Acquire) {
        let timer_delay = driver
            .state
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
        if driver.handle_event(event, now, &mut client_timer)? {
            return Ok(());
        }
    }
    // Clean exit (Ctrl+Q). Send Detach before closing.
    let _ = write_to_server(&mut driver.write_stream, &ClientMessage::Detach);
    let _ = io::stdout().flush();
    Ok(())
}

#[derive(Clone, Copy)]
struct InitialClientGeometry {
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
    pixel_geometry_exact: bool,
}

struct ClientLoopDriver {
    // Disconnect before releasing the shell, as the original loop locals did.
    write_stream: endpoint::ServerConnection,
    endpoint_commands: endpoint_commands::EndpointCommands,
    host_sgr_pixels_active: Arc<AtomicBool>,
    #[cfg(unix)]
    reported_cell_size: Arc<AtomicU64>,
    host_mouse_capture_active: Arc<AtomicBool>,
    state: ClientState,
    #[cfg(unix)]
    will_query_host_cell_size: bool,
}

fn start_client_loop(
    initial: Option<(LocalStream, handshake::HandshakeResult)>,
    geometry: InitialClientGeometry,
    should_quit: &Arc<AtomicBool>,
    input_lifecycle: ClientInputLifecycle,
    config: ClientLoopConfig,
    event_tx: &tokio::sync::mpsc::Sender<ClientLoopEvent>,
    stdin_tx: tokio::sync::mpsc::Sender<ClientLoopEvent>,
) -> Result<ClientLoopDriver, ClientError> {
    let pixel_geometry_fallback = config.pixel_geometry_fallback;
    let (state, stdin_quit) = initial_client_state(config, input_lifecycle, geometry)?;
    let host_mouse_capture_active = Arc::new(AtomicBool::new(state.mouse_capture_active));
    // Cell size reported by the host terminal, packed as width<<32 | height.
    // Zero means the host has not reported one.
    let reported_cell_size = Arc::new(AtomicU64::new(0));
    let host_sgr_pixels_active = Arc::new(AtomicBool::new(false));
    let endpoint_commands = endpoint_commands::EndpointCommands::default();
    let cell_size_query = start_host_readers(
        &state,
        geometry,
        should_quit,
        stdin_quit,
        pixel_geometry_fallback,
        event_tx,
        stdin_tx,
        &host_mouse_capture_active,
        &reported_cell_size,
        &host_sgr_pixels_active,
    );
    #[cfg(windows)]
    let _ = cell_size_query; // Windows parses host geometry in its stdin reader.
    let (stream, _handshake) = initial.expect("connected client");
    let max_frame_size = if state.kitty_graphics_enabled {
        MAX_GRAPHICS_FRAME_SIZE
    } else {
        crate::protocol::MAX_FRAME_SIZE
    };
    let transport = start_endpoint_transport(stream, (), event_tx, max_frame_size)?;
    let mut write_stream = endpoint::ServerConnection::new(transport);
    if state.shell.is_some() {
        write_stream
            .send(&ClientMessage::ClientShellFocus { focused: true })
            .map_err(ClientError::ConnectionLost)?;
    }

    Ok(ClientLoopDriver {
        state,
        write_stream,
        endpoint_commands,
        host_mouse_capture_active,
        #[cfg(unix)]
        reported_cell_size,
        host_sgr_pixels_active,
        #[cfg(unix)]
        will_query_host_cell_size: cell_size_query,
    })
}

fn initial_client_state(
    config: ClientLoopConfig,
    input_lifecycle: ClientInputLifecycle,
    geometry: InitialClientGeometry,
) -> Result<(ClientState, Arc<AtomicBool>), ClientError> {
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
        reported_size: (geometry.cols, geometry.rows),
        reported_cell_size: (geometry.cell_width_px, geometry.cell_height_px),
        sound_config: config.sound_config,
        kitty_graphics_enabled: config.kitty_graphics_enabled,
        pixel_geometry_enabled: config.pixel_geometry_enabled,
        pixel_geometry_exact: geometry.pixel_geometry_exact,
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
        shell.set_graphics_cell_size(geometry.cell_width_px, geometry.cell_height_px);
        shell.set_bus_kitty_graphics(state.kitty_graphics_enabled);
    }
    Ok((state, input_lifecycle.reader_should_quit))
}

fn start_host_readers(
    state: &ClientState,
    geometry: InitialClientGeometry,
    should_quit: &Arc<AtomicBool>,
    stdin_quit: Arc<AtomicBool>,
    pixel_geometry_fallback: bool,
    event_tx: &tokio::sync::mpsc::Sender<ClientLoopEvent>,
    stdin_tx: tokio::sync::mpsc::Sender<ClientLoopEvent>,
    host_mouse_capture_active: &Arc<AtomicBool>,
    reported_cell_size: &Arc<AtomicU64>,
    host_sgr_pixels_active: &Arc<AtomicBool>,
) -> bool {
    let InitialClientGeometry {
        cols,
        rows,
        cell_width_px: initial_cell_width_px,
        cell_height_px: initial_cell_height_px,
        pixel_geometry_exact: initial_pixel_geometry_exact,
    } = geometry;
    // Spawn the stdin reader thread.
    let will_query_host_terminal_theme = should_query_host_terminal_theme();
    // Terminals behind ConPTY report no pixel size through the ioctl, so ask the
    // host terminal directly instead of falling back to an assumed cell size.
    let will_query_host_cell_size = host_cell_size_query_required(state.kitty_graphics_enabled);

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

    will_query_host_cell_size
}

impl ClientLoopDriver {
    fn handle_event(
        &mut self,
        event: ClientLoopEvent,
        now: std::time::Instant,
        client_timer: &mut timer::ClientLoopTimer,
    ) -> Result<bool, ClientError> {
        match event {
            #[cfg(unix)]
            ClientLoopEvent::StdinInput(data) => {
                if self.state.shell.is_some() && self.will_query_host_cell_size {
                    let events = crate::raw_input::parse_raw_input_bytes_sync(&data);
                    if let Some((width_px, height_px)) = reported_cell_size_from_events(&events) {
                        store_reported_cell_size(&self.reported_cell_size, width_px, height_px);
                    }
                }
                return self.apply_shell_input(|shell| shell.handle_input_bytes(&data));
            }
            #[cfg(unix)]
            ClientLoopEvent::PixelMouse(data, geometry) => {
                return self.apply_shell_input(|shell| shell.handle_pixel_mouse(&data, geometry));
            }
            #[cfg(windows)]
            ClientLoopEvent::StdinEvents(events) => {
                return self.apply_shell_input(|shell| shell.handle_client_events(&events));
            }
            ClientLoopEvent::TerminalUnavailable(err) => {
                info!(err = %err, "client terminal unavailable; detaching");
                let _ = write_to_server(&mut self.write_stream, &ClientMessage::Detach);
                return Ok(true);
            }
            ClientLoopEvent::Resize(
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                pixel_geometry_exact,
            ) => {
                self.resize(
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                    pixel_geometry_exact,
                )?;
            }
            ClientLoopEvent::ServerMessage { message } => {
                if self.write_stream.is_connected() {
                    return server_messages::handle_server_message(
                        message,
                        &mut self.state,
                        &mut self.write_stream,
                        &mut self.endpoint_commands,
                        &self.host_mouse_capture_active,
                        &self.host_sgr_pixels_active,
                    );
                }
            }
            ClientLoopEvent::ServerDisconnected => {
                self.write_stream.fail(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection was lost",
                ));
            }
            ClientLoopEvent::Timer => {
                client_timer.fired();
                return self.tick(now);
            }
        }
        Ok(false)
    }

    fn apply_shell_input(
        &mut self,
        input: impl FnOnce(&mut shell::ClientShellState) -> shell::ClientShellInput,
    ) -> Result<bool, ClientError> {
        let Some(shell) = self.state.shell.as_mut() else {
            return Ok(false);
        };
        let outcome = input(shell);
        let frame = outcome
            .repaint
            .then(|| shell.compose(self.state.reported_size.0, self.state.reported_size.1))
            .flatten();
        finish_client_shell_input(
            &mut self.state,
            outcome,
            frame,
            &mut self.write_stream,
            &mut self.endpoint_commands,
        )
    }

    fn resize(
        &mut self,
        new_cols: u16,
        new_rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_geometry_exact: bool,
    ) -> Result<(), ClientError> {
        let state = &mut self.state;
        let host_sgr_pixels_active = &self.host_sgr_pixels_active;
        let write_stream = &mut self.write_stream;
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
        if let Err(e) = write_to_server(write_stream, &msg) {
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
        Ok(())
    }

    fn tick(&mut self, now: std::time::Instant) -> Result<bool, ClientError> {
        let state = &mut self.state;
        let write_stream = &mut self.write_stream;
        state
            .detached_process_children
            .retain_mut(|child| child.try_wait().ok().flatten().is_none());
        if let Some(error) = write_stream.take_failure() {
            warn!(error = %error, "server connection failed");
            return Err(ClientError::ConnectionLost(error));
        }

        if self.state.shell.is_none() {
            return Ok(false);
        }
        let expired = self.endpoint_commands.expire(now);
        self.apply_shell_input(|shell| {
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
            outcome
        })
    }
}
