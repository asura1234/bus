#[cfg(not(windows))]
use super::query_host_terminal_appearance;
use super::{
    endpoint, endpoint_commands, query_host_terminal_theme, shell, write_to_server, ClientError,
    ClientState,
};
#[cfg(test)]
use crate::protocol::render_ansi;
use crate::protocol::{ClientMessage, FrameData};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::Arc;
use tracing::warn;

/// Set when the human quits Bus. The server and its agents stop only after the
/// client has restored the terminal, so the stop wait and any error stay readable.
static STOP_SERVER_AFTER_QUIT: AtomicBool = AtomicBool::new(false);

/// Runs `stop` once if Bus was quit; a plain connection loss leaves the server alone.
pub(super) fn stop_server_after_quit(
    stop: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if STOP_SERVER_AFTER_QUIT.swap(false, Ordering::AcqRel) {
        stop()
    } else {
        Ok(())
    }
}

pub(super) fn dispatch_client_shell_actions(
    actions: Vec<shell::ClientShellAction>,
    endpoint_commands: &mut endpoint_commands::EndpointCommands,
    connection: &mut endpoint::ServerConnection,
    mut shell: Option<&mut shell::ClientShellState>,
    detached_process_children: &mut Vec<std::process::Child>,
) -> Result<(Vec<crossterm::event::MouseEvent>, bool), ClientError> {
    let mut replay_mouse = Vec::new();
    let mut repaint = false;
    for action in actions {
        match action {
            shell::ClientShellAction::Endpoint { boot_id, request } => {
                endpoint_commands.enqueue(boot_id, request);
            }
            shell::ClientShellAction::ClipboardWrite(bytes) => {
                crate::client::clipboard::write_osc52_bytes(&bytes);
            }
            shell::ClientShellAction::EditComposer => {
                if let Some(shell) = shell.as_deref_mut() {
                    if let Err(error) = shell.edit_bus_composer() {
                        tracing::warn!(error = %error, "bus composer editor failed");
                    }
                    repaint = true;
                }
            }
            shell::ClientShellAction::OpenSafeWebUrl(url) => {
                if crate::app::actions::safe_web_url(&url).is_some() {
                    match crate::platform::open_url(&url) {
                        Ok(Some(child)) => detached_process_children.push(child),
                        Ok(None) => {}
                        Err(err) => warn!(err = %err, url = %url, "failed to open pane URL"),
                    }
                }
            }
            shell::ClientShellAction::ReplayMouse(events) => replay_mouse.extend(events),
        }
    }
    let cancelled = endpoint_commands.send_next(connection);
    if let Some(shell) = shell {
        for request_id in cancelled {
            repaint |= shell.cancel_endpoint_request(&request_id);
        }
    }
    Ok((replay_mouse, repaint))
}

pub(super) fn client_shell_resize_message(
    shell: &shell::ClientShellState,
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
    pixel_mouse: bool,
) -> ClientMessage {
    ClientMessage::ClientShellResize {
        cell_width_px,
        cell_height_px,
        surface_size: shell.surface_size(cols, rows),
        pixel_mouse,
    }
}

pub(super) fn sync_client_shell_keyboard_report_all(
    state: &mut ClientState,
) -> Result<(), ClientError> {
    if state.shell.is_none() {
        return Ok(());
    }
    let desired = state.pane_keyboard_report_all;
    if desired == state.keyboard_report_all_active {
        return Ok(());
    }
    crate::terminal_modes::set_host_kitty_keyboard_report_all(&mut io::stdout(), desired)
        .map_err(ClientError::ConnectionFailed)?;
    state.keyboard_report_all_active = desired;
    Ok(())
}

pub(super) use crate::client::compositor::snapshot::install_client_shell_snapshot;

pub(super) fn finish_client_shell_input(
    state: &mut ClientState,
    outcome: shell::ClientShellInput,
    frame: Option<FrameData>,
    connection: &mut endpoint::ServerConnection,
    endpoint_commands: &mut endpoint_commands::EndpointCommands,
) -> Result<bool, ClientError> {
    if outcome.detach {
        // Only Bus quits a shell client, and quitting Bus ends the whole session
        // like `bus stop`: Bus state is already saved, the server saves the rest.
        STOP_SERVER_AFTER_QUIT.store(true, Ordering::Release);
        let _ = write_to_server(connection, &ClientMessage::Detach);
        return Ok(true);
    }
    if outcome.resize {
        let shell = state.shell.as_ref().expect("shell mode remains active");
        let resize = client_shell_resize_message(
            shell,
            state.reported_size.0,
            state.reported_size.1,
            state.reported_cell_size.0,
            state.reported_cell_size.1,
            state.pixel_geometry_exact,
        );
        write_to_server(connection, &resize).map_err(ClientError::ConnectionLost)?;
    }
    #[cfg(not(windows))]
    if outcome.query_host_appearance {
        query_host_terminal_appearance();
    }
    if outcome.query_host_theme {
        query_host_terminal_theme(
            &state.host_palette_query_pending,
            &state.host_palette_query_progress,
        );
    }
    sync_client_shell_keyboard_report_all(state)?;
    let (replay, dispatch_repaint) = dispatch_client_shell_actions(
        outcome.actions,
        endpoint_commands,
        connection,
        state.shell.as_mut(),
        &mut state.detached_process_children,
    )?;
    let frame = if dispatch_repaint {
        state
            .shell
            .as_mut()
            .and_then(|shell| shell.compose(state.reported_size.0, state.reported_size.1))
    } else {
        frame
    };
    debug_assert!(
        replay.is_empty(),
        "mouse replay only follows endpoint results"
    );
    // Shell requests address panes in the server's snapshot, so none go out before the first one.
    if state
        .shell
        .as_ref()
        .is_none_or(|shell| shell.has_snapshot())
    {
        for request in outcome.requests {
            write_to_server(connection, &request).map_err(ClientError::ConnectionLost)?;
        }
    }
    if let Some(frame) = frame {
        state.present_frame(frame);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct RecordingTransport(Arc<Mutex<Vec<ClientMessage>>>);

    impl endpoint::EndpointTransport for RecordingTransport {
        fn send(&mut self, message: &ClientMessage) -> io::Result<()> {
            self.0.lock().unwrap().push(message.clone());
            Ok(())
        }
    }

    fn client_state() -> ClientState {
        ClientState {
            blit_encoder: render_ansi::BlitEncoder::new(),
            mouse_capture_active: false,
            endpoint_mouse_capture_requested: false,
            endpoint_sgr_pixels_requested: false,
            host_palette_query_pending: Arc::default(),
            host_palette_query_progress: Arc::default(),
            shell_mouse_capture_preference: false,
            pane_keyboard_report_all: false,
            keyboard_report_all_active: false,
            reported_size: (80, 24),
            reported_cell_size: (0, 0),
            sound_config: crate::config::SoundConfig::default(),
            kitty_graphics_enabled: false,
            pixel_geometry_enabled: false,
            pixel_geometry_exact: false,
            redraw_on_focus_gained: false,
            repaint_pending: false,
            draw_host_cursor: false,
            detached_process_children: Vec::new(),
            shell: None,
        }
    }

    #[test]
    fn quitting_bus_detaches_then_stops_the_server_once() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut connection = endpoint::ServerConnection::new(RecordingTransport(sent.clone()));
        let mut commands = endpoint_commands::EndpointCommands::default();
        let mut stops = 0;
        // Nothing quit yet: losing the connection must not stop the server.
        stop_server_after_quit(|| {
            stops += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(stops, 0);

        let outcome = shell::ClientShellInput {
            detach: true,
            ..Default::default()
        };
        let exit = finish_client_shell_input(
            &mut client_state(),
            outcome,
            None,
            &mut connection,
            &mut commands,
        )
        .unwrap();
        assert!(exit, "a Bus quit ends the client loop");
        assert!(matches!(
            sent.lock().unwrap().as_slice(),
            [ClientMessage::Detach]
        ));
        for _ in 0..2 {
            stop_server_after_quit(|| {
                stops += 1;
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(stops, 1, "one quit stops the server exactly once");
    }
}
