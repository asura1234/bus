use super::{
    client_shell_resize_message, endpoint, should_draw_host_cursor, write_to_server, ClientError,
    ClientState,
};
use crate::protocol::ClientMessage;
use tracing::{debug, warn};

pub(super) fn init_logging(options: &crate::utils::logging::LoggingOptions) {
    crate::utils::logging::init_file_logging_at(
        crate::utils::paths::data_dir(),
        "herdr-client.log",
        options,
    );
}

pub(super) fn apply_reload(
    state: &mut ClientState,
    connection: &mut endpoint::ServerConnection,
) -> Result<(), ClientError> {
    let previous_mouse_capture = state.shell_mouse_capture_preference;
    let mut mouse_capture = previous_mouse_capture;
    reload_local_client_config(
        &mut state.sound_config,
        &mut state.redraw_on_focus_gained,
        &mut state.draw_host_cursor,
        &mut mouse_capture,
    );
    state.shell_mouse_capture_preference = mouse_capture;
    if state.shell.is_some() && previous_mouse_capture != mouse_capture {
        write_to_server(
            connection,
            &ClientMessage::ClientShellMouseCapture {
                enabled: mouse_capture,
            },
        )
        .map_err(ClientError::ConnectionLost)?;
    }
    let (frame, resize) = if let Some(shell) = state.shell.as_mut() {
        let previous_size = shell.surface_size(state.reported_size.0, state.reported_size.1);
        shell.reload_client_config();
        let next_size = shell.surface_size(state.reported_size.0, state.reported_size.1);
        let resize = (previous_size != next_size).then(|| {
            shell.invalidate_pane_surface();
            client_shell_resize_message(
                shell,
                state.reported_size.0,
                state.reported_size.1,
                state.reported_cell_size.0,
                state.reported_cell_size.1,
                state.pixel_geometry_exact,
            )
        });
        (
            shell.compose(state.reported_size.0, state.reported_size.1),
            resize,
        )
    } else {
        (None, None)
    };
    if let Some(resize) = resize {
        write_to_server(connection, &resize).map_err(ClientError::ConnectionLost)?;
    }
    if let Some(frame) = frame {
        state.present_frame(frame);
    }
    Ok(())
}

pub(super) fn reload_local_client_config(
    sound_config: &mut crate::config::SoundConfig,
    redraw_on_focus_gained: &mut bool,
    draw_host_cursor: &mut bool,
    mouse_capture: &mut bool,
) {
    match crate::config::load_live_config() {
        Ok(loaded) => {
            let invalid_section = |section: &str| {
                loaded
                    .invalid_sections
                    .iter()
                    .any(|invalid| invalid == section)
            };
            if !invalid_section("ui") {
                for diagnostic in loaded.config.ui.sound.diagnostics() {
                    warn!(diagnostic = %diagnostic, "local sound config diagnostic");
                }
                *sound_config = loaded.config.ui.sound.clone();
                *redraw_on_focus_gained = loaded.config.ui.redraw_on_focus_gained;
                *draw_host_cursor = should_draw_host_cursor(loaded.config.ui.host_cursor);
                *mouse_capture = loaded.config.ui.mouse_capture;
            }
            debug!("reloaded local client config");
        }
        Err(diagnostics) => {
            warn!(diagnostics = ?diagnostics, "failed to reload local client config; keeping current client config");
        }
    }
}
