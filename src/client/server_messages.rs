//! Server messages applied to the foreground client's presentation and effects.
use super::*;

/// Returns true when message handling requests a normal client-loop exit.
pub(super) fn handle_server_message(
    message: Box<ServerMessage>,
    state: &mut ClientState,
    write_stream: &mut endpoint::ServerConnection,
    endpoint_commands: &mut endpoint_commands::EndpointCommands,
    host_mouse_capture_active: &AtomicBool,
    host_sgr_pixels_active: &AtomicBool,
) -> Result<bool, ClientError> {
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
            crate::render_prof::duration_since("client_surface_patch.apply", apply_started);
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
                let composed = state
                    .shell
                    .as_mut()
                    .and_then(|shell| shell.compose(state.reported_size.0, state.reported_size.1));
                if let Some(frame) = composed {
                    state.present_frame(frame);
                }
            }
            crate::render_prof::duration_since("client_surface_patch.total", patch_started);
            crate::render_prof::flush_if_due();
        }
        ServerMessage::Terminal(frame) => {
            if state.kitty_graphics_enabled && contains_kitty_graphics_bytes(&frame.bytes) {
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
        ServerMessage::GraphicsFile { .. } | ServerMessage::GraphicsTransmissionRetired { .. } => {}
        ServerMessage::TerminalBell { count } => {
            let bus_shell = state.shell.as_ref().is_some_and(|shell| shell.has_bus());
            if let Err(err) = forward_terminal_bells(&mut io::stdout(), count, bus_shell) {
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
                handle_notify(kind, &message, body.as_deref());
            }
        }
        ServerMessage::SemanticNotification(_) => {}
        ServerMessage::ClientShellError { message } => {
            warn!(%message, "server rejected client shell input");
        }
        ServerMessage::ClientShellEndpointResponseChunk {
            boot_id,
            request_id,
            final_chunk,
            data,
        } => {
            if request_id.starts_with("client-shell-surface:") {
                return Ok(false);
            }
            let Some(completed) =
                endpoint_commands.receive_chunk(&boot_id, &request_id, final_chunk, data)
            else {
                return Ok(false);
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
                endpoint_commands,
                write_stream,
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
                        .then(|| shell.compose(state.reported_size.0, state.reported_size.1))
                        .flatten();
                    (outcome, frame)
                };
                if finish_client_shell_input(
                    state,
                    outcome,
                    frame,
                    write_stream,
                    endpoint_commands,
                )? {
                    return Ok(true);
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
            let _ =
                crate::terminal_effects::write_window_title(&mut io::stdout(), title.as_deref());
        }
        ServerMessage::ReloadSoundConfig => apply_reload(state, write_stream)?,
        ServerMessage::MouseCapture {
            enabled,
            sgr_pixels,
        } => {
            state.endpoint_mouse_capture_requested = enabled;
            state.endpoint_sgr_pixels_requested = sgr_pixels;
            let next_sgr_pixels =
                effective_sgr_pixel_mouse(enabled, sgr_pixels, state.pixel_geometry_exact);
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
                sync_client_shell_keyboard_report_all(state)?;
            }
        }
        ServerMessage::EndpointControl { kind, data } => {
            let snapshot = match endpoint::decode_endpoint_control(&kind, &data) {
                Ok(endpoint::EndpointControlMessage::Ignored) => {
                    debug!(%kind, "ignoring unknown endpoint control message");
                    return Ok(false);
                }
                Ok(endpoint::EndpointControlMessage::Snapshot(snapshot)) => snapshot,
                Err(message) => {
                    return Err(ClientError::Protocol(protocol::FramingError::Io(
                        io::Error::new(io::ErrorKind::InvalidData, message),
                    )));
                }
            };
            install_client_shell_snapshot(state, snapshot, write_stream)?;
        }
        ServerMessage::Welcome { .. } => {
            debug!("received unexpected Welcome in main loop");
        }
    }
    Ok(false)
}
