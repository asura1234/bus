use super::*;

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
                crate::selection::write_osc52_bytes(&bytes);
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

pub(super) fn install_client_shell_snapshot(
    state: &mut ClientState,
    snapshot: Box<crate::protocol::ClientShellSnapshot>,
    connection: &mut endpoint::ServerConnection,
) -> Result<(), ClientError> {
    let Some(shell) = state.shell.as_mut() else {
        return Ok(());
    };
    let previous_size = shell.surface_size(state.reported_size.0, state.reported_size.1);
    shell.install_snapshot(snapshot);
    let graphics_cleanup = shell.take_pending_graphics_cleanup();
    let next_size = shell.surface_size(state.reported_size.0, state.reported_size.1);
    let composed = shell.compose(state.reported_size.0, state.reported_size.1);
    let resize = (previous_size != next_size).then(|| {
        client_shell_resize_message(
            shell,
            state.reported_size.0,
            state.reported_size.1,
            state.reported_cell_size.0,
            state.reported_cell_size.1,
            state.pixel_geometry_exact,
        )
    });
    state.present_graphics(&graphics_cleanup);
    if let Some(resize) = resize {
        write_to_server(connection, &resize).map_err(ClientError::ConnectionLost)?;
    }
    if let Some(frame) = composed {
        state.present_frame(frame);
    }
    Ok(())
}

pub(super) fn finish_client_shell_input(
    state: &mut ClientState,
    outcome: shell::ClientShellInput,
    frame: Option<FrameData>,
    connection: &mut endpoint::ServerConnection,
    endpoint_commands: &mut endpoint_commands::EndpointCommands,
) -> Result<bool, ClientError> {
    if outcome.detach {
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
