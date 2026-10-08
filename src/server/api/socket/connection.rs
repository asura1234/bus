//! Decode one API request, dispatch it, or serve its wait/subscription stream.
#[cfg(test)]
use super::accept::default_capabilities;
use super::CONNECTION_POLL_INTERVAL;
use crate::platform::ipc::{
    is_connection_closed_error, local_stream_peer_closed, poll_local_stream_read,
    set_local_stream_polling, LocalStream, LocalStreamRead,
};
use crate::protocol::api::schema::{
    ErrorBody, ErrorResponse, Method, Request, ResponseResult, ServerCapabilities, SuccessResponse,
};
use crate::server::api::streams::subscriptions::ActiveSubscription;
use crate::server::api::streams::wait::{
    prompt_agent, wait_for_agent, wait_for_event, wait_for_output,
};
use crate::server::api::{request_changes_ui, ApiRequestMessage, ApiRequestSender, EventHub};
use interprocess::local_socket::traits::Stream as _;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::debug;

const INITIAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const STREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_INITIAL_REQUEST_BYTES: usize = 1024 * 1024;

#[cfg(test)]
pub(super) fn handle_connection(
    stream: LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<()> {
    handle_connection_with_stop(
        stream,
        api_tx,
        event_hub,
        running,
        default_capabilities(),
        &Arc::new(AtomicBool::new(false)),
    )
}

pub(super) fn handle_connection_with_stop(
    mut stream: LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
    capabilities: ServerCapabilities,
    server_stop: &Arc<AtomicBool>,
) -> std::io::Result<()> {
    if let Err(err) = stream.set_send_timeout(Some(STREAM_WRITE_TIMEOUT)) {
        debug!(target: "bus::server::api::socket", err = %err, "api connection write timeout unavailable");
    }

    let Some(line) = read_initial_request_line(&mut stream)? else {
        return Ok(());
    };

    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }

    let request = match serde_json::from_str::<Request>(line) {
        Ok(request) => request,
        Err(request_error) => {
            write_json_line_allow_disconnect(
                &mut stream,
                &ErrorResponse {
                    id: String::new(),
                    error: ErrorBody {
                        code: "invalid_request".into(),
                        message: format!("invalid request: {request_error}"),
                    },
                },
            )?;
            return Ok(());
        }
    };

    let request_id = request.id.clone();
    let method = api_method_name(&request.method);
    let changes_ui = request_changes_ui(&request);
    crate::utils::logging::api_request_started(&request_id, method, changes_ui);

    match request.method {
        Method::EventsSubscribe(params) => {
            let result = stream_subscriptions(
                stream,
                request_id.clone(),
                params,
                api_tx,
                event_hub,
                running,
            );
            match &result {
                Ok(()) => crate::utils::logging::api_request_completed(
                    &request_id,
                    method,
                    "stream_closed",
                    changes_ui,
                ),
                Err(err) => {
                    crate::utils::logging::api_request_failed(&request_id, method, &err.to_string())
                }
            }
            result
        }
        Method::EventsWait(params) => {
            let response = wait_for_event(
                request_id.clone(),
                params,
                &mut stream,
                api_tx,
                event_hub,
                running,
            )?;
            finish_wait_response(&mut stream, response, &request_id, method, changes_ui)
        }
        Method::AgentPrompt(params) => {
            let response = prompt_agent(
                request_id.clone(),
                params,
                &mut stream,
                api_tx,
                event_hub,
                running,
            )?;
            finish_wait_response(&mut stream, response, &request_id, method, changes_ui)
        }
        Method::AgentWait(params) => {
            let response = wait_for_agent(
                request_id.clone(),
                params,
                &mut stream,
                api_tx,
                event_hub,
                running,
            )?;
            finish_wait_response(&mut stream, response, &request_id, method, changes_ui)
        }
        Method::PaneWaitForOutput(params) => {
            let response =
                wait_for_output(request_id.clone(), params, &mut stream, api_tx, running)?;
            finish_wait_response(&mut stream, response, &request_id, method, changes_ui)
        }
        method_body => {
            let response = handle_request(
                Request {
                    id: request_id.clone(),
                    method: method_body,
                },
                api_tx,
                capabilities,
                server_stop,
            );
            let result = write_text_line_allow_disconnect(&mut stream, &response);
            match &result {
                Ok(()) => crate::utils::logging::api_request_completed(
                    &request_id,
                    method,
                    api_response_outcome(&response),
                    changes_ui,
                ),
                Err(err) => {
                    crate::utils::logging::api_request_failed(&request_id, method, &err.to_string())
                }
            }
            result
        }
    }
}

fn finish_wait_response(
    stream: &mut LocalStream,
    response: Option<String>,
    request_id: &str,
    method: &'static str,
    changes_ui: bool,
) -> std::io::Result<()> {
    let Some(response) = response else {
        crate::utils::logging::api_request_completed(
            request_id,
            method,
            "client_disconnected",
            changes_ui,
        );
        return Ok(());
    };
    let result = write_text_line_allow_disconnect(stream, &response);
    match &result {
        Ok(()) => crate::utils::logging::api_request_completed(
            request_id,
            method,
            api_response_outcome(&response),
            changes_ui,
        ),
        Err(err) => crate::utils::logging::api_request_failed(request_id, method, &err.to_string()),
    }
    result
}

pub(super) fn handle_request(
    request: Request,
    api_tx: &ApiRequestSender,
    capabilities: ServerCapabilities,
    server_stop: &Arc<AtomicBool>,
) -> String {
    if matches!(&request.method, Method::Ping(_)) {
        return serde_json::to_string(&SuccessResponse {
            id: request.id,
            result: ResponseResult::Pong {
                version: crate::utils::version::version(),
                protocol: crate::protocol::PROTOCOL_VERSION,
                capabilities: Some(capabilities),
            },
        })
        .unwrap_or_else(|_| {
            r#"{"id":"","error":{"code":"internal_error","message":"failed to encode response"}}"#
                .to_string()
        });
    }

    if matches!(&request.method, Method::ClientShellSurfaceSet(_)) {
        return error_response_json(
            request.id,
            "connection_local_only",
            "client_shell.surface.set is only available through a client shell endpoint".into(),
        );
    }

    if matches!(&request.method, Method::ServerStop(_)) {
        server_stop.store(true, Ordering::Release);
        return serde_json::to_string(&SuccessResponse {
            id: request.id,
            result: ResponseResult::Ok {},
        })
        .unwrap_or_else(|_| "{}".to_string());
    } else if server_stop.load(Ordering::Acquire) {
        return error_response_json(
            request.id,
            "server_unavailable",
            "server is shutting down".into(),
        );
    }

    dispatch_to_app(request, api_tx, None, None)
}

pub(crate) fn api_method_name(method: &Method) -> &'static str {
    match method {
        Method::Ping(_) => "ping",
        Method::ServerStop(_) => "server.stop",
        Method::ServerReloadConfig(_) => "server.reload_config",
        Method::NotificationShow(_) => "notification.show",
        Method::ClientWindowTitleSet(_) => "client.window_title.set",
        Method::ClientWindowTitleClear(_) => "client.window_title.clear",
        Method::ClientShellSurfaceSet(_) => "client_shell.surface.set",
        Method::SessionSnapshot(_) => "session.snapshot",
        Method::WorkspaceCreate(_) => "workspace.create",
        Method::WorkspaceList(_) => "workspace.list",
        Method::WorkspaceGet(_) => "workspace.get",
        Method::WorkspaceFocus(_) => "workspace.focus",
        Method::WorkspaceRename(_) => "workspace.rename",
        Method::WorkspaceMove(_) => "workspace.move",
        Method::WorkspaceClose(_) => "workspace.close",
        Method::TabCreate(_) => "tab.create",
        Method::TabList(_) => "tab.list",
        Method::TabGet(_) => "tab.get",
        Method::TabFocus(_) => "tab.focus",
        Method::TabRename(_) => "tab.rename",
        Method::TabClose(_) => "tab.close",
        Method::AgentList(_) => "agent.list",
        Method::AgentGet(_) => "agent.get",
        Method::AgentRead(_) => "agent.read",
        Method::AgentDialogObserve(_) => "agent.dialog.observe",
        Method::AgentDialogChoose(_) => "agent.dialog.choose",
        Method::AgentDialogAnswer(_) => "agent.dialog.answer",
        Method::AgentSendKeys(_) => "agent.send_keys",
        Method::AgentRename(_) => "agent.rename",
        Method::AgentFocus(_) => "agent.focus",
        Method::AgentStart(_) => "agent.start",
        Method::AgentPrompt(_) => "agent.prompt",
        Method::AgentPromptIfIdle(_) => "agent.prompt_if_idle",
        Method::AgentPromptIfUnbound(_) => "agent.prompt_if_unbound",
        Method::AgentWait(_) => "agent.wait",
        Method::PaneSplit(_) => "pane.split",
        Method::PaneSwap(_) => "pane.swap",
        Method::PaneZoom(_) => "pane.zoom",
        Method::PaneLayout(_) => "pane.layout",
        Method::LayoutSetSplitRatio(_) => "layout.set_split_ratio",
        Method::PaneFocusDirection(_) => "pane.focus_direction",
        Method::PaneResize(_) => "pane.resize",
        Method::PaneScroll(_) => "pane.scroll",
        Method::PaneSelectionRead(_) => "pane.selection.read",
        Method::PaneCopyMotion(_) => "pane.copy_motion",
        Method::PaneCopySearch(_) => "pane.copy_search",
        Method::PaneList(_) => "pane.list",
        Method::PaneCurrent(_) => "pane.current",
        Method::PaneGet(_) => "pane.get",
        Method::PaneFocus(_) => "pane.focus",
        Method::PaneInputSet(_) => "pane.input.set",
        Method::PaneLinkActivate(_) => "pane.link.activate",
        Method::PaneRename(_) => "pane.rename",
        Method::PaneSendText(_) => "pane.send_text",
        Method::PaneSendKeys(_) => "pane.send_keys",
        Method::PaneSendInput(_) => "pane.send_input",
        Method::PaneRead(_) => "pane.read",
        Method::PaneReportAgentSession(_) => "pane.report_agent_session",
        Method::PaneClose(_) => "pane.close",
        Method::PaneCloseIfIdentity(_) => "pane.close_if_identity",
        Method::EventsSubscribe(_) => "events.subscribe",
        Method::EventsWait(_) => "events.wait",
        Method::PaneWaitForOutput(_) => "pane.wait_for_output",
    }
}

pub(super) fn api_response_outcome(response: &str) -> &'static str {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(response) else {
        return "error";
    };

    match value
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(|code| code.as_str())
    {
        Some("timeout") => "timeout",
        Some(_) => "error",
        None => "ok",
    }
}

fn read_initial_request_line(stream: &mut LocalStream) -> std::io::Result<Option<String>> {
    read_initial_request_line_with_timeout(stream, INITIAL_REQUEST_TIMEOUT)
}

pub(super) fn read_initial_request_line_with_timeout(
    stream: &mut LocalStream,
    timeout: Duration,
) -> std::io::Result<Option<String>> {
    read_initial_request_line_with_limits(stream, timeout, MAX_INITIAL_REQUEST_BYTES)
}

pub(super) fn read_initial_request_line_with_limits(
    stream: &mut LocalStream,
    timeout: Duration,
    max_bytes: usize,
) -> std::io::Result<Option<String>> {
    set_local_stream_polling(stream, true)?;
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];

    let result = loop {
        let read = match poll_local_stream_read(stream, &mut byte) {
            Ok(read) => read,
            Err(err) => break Err(err),
        };
        match read {
            LocalStreamRead::Closed => break Ok(None),
            LocalStreamRead::Data => {
                bytes.push(byte[0]);
                if byte[0] == b'\n' {
                    break String::from_utf8(bytes)
                        .map(Some)
                        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err));
                }
                if bytes.len() > max_bytes {
                    break Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "api request line is too large",
                    ));
                }
            }
            LocalStreamRead::Pending => {
                if Instant::now() >= deadline {
                    break Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out reading api request",
                    ));
                }
                std::thread::sleep(CONNECTION_POLL_INTERVAL);
            }
        }
    };
    set_local_stream_polling(stream, false)?;
    result
}

fn stream_subscriptions(
    mut stream: LocalStream,
    request_id: String,
    params: crate::protocol::api::schema::EventsSubscribeParams,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<()> {
    let event_start_sequence = event_hub.current_sequence();
    let mut subscriptions = Vec::with_capacity(params.subscriptions.len());
    for (index, subscription) in params.subscriptions.into_iter().enumerate() {
        let active = match ActiveSubscription::new(
            subscription,
            &request_id,
            index,
            api_tx,
            event_hub,
            event_start_sequence,
        ) {
            Ok(active) => active,
            Err(response) => {
                if let Err(err) = write_json_line(&mut stream, &response) {
                    if is_connection_closed_error(&err) {
                        return Ok(());
                    }
                    return Err(err);
                }
                return Ok(());
            }
        };
        subscriptions.push(active);
    }

    if let Err(err) = write_json_line(
        &mut stream,
        &SuccessResponse {
            id: request_id,
            result: ResponseResult::SubscriptionStarted {},
        },
    ) {
        if is_connection_closed_error(&err) {
            return Ok(());
        }
        return Err(err);
    }

    loop {
        if should_stop_connection(&mut stream, running)? {
            return Ok(());
        }

        for subscription in &mut subscriptions {
            if let Some(event) = subscription.poll(api_tx, event_hub) {
                if let Err(err) = write_json_line(&mut stream, &event) {
                    if is_connection_closed_error(&err) {
                        return Ok(());
                    }
                    return Err(err);
                }
            }
        }
        std::thread::sleep(CONNECTION_POLL_INTERVAL);
    }
}

fn write_text_line(stream: &mut LocalStream, value: &str) -> std::io::Result<()> {
    stream.write_all(value.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn write_text_line_allow_disconnect(stream: &mut LocalStream, value: &str) -> std::io::Result<()> {
    match write_text_line(stream, value) {
        Err(err) if is_connection_closed_error(&err) => Ok(()),
        result => result,
    }
}

fn write_json_line<T: serde::Serialize>(
    stream: &mut LocalStream,
    value: &T,
) -> std::io::Result<()> {
    let encoded = serde_json::to_string(value)
        .map_err(|err| std::io::Error::other(format!("failed to encode json: {err}")))?;
    write_text_line(stream, &encoded)
}

fn write_json_line_allow_disconnect<T: serde::Serialize>(
    stream: &mut LocalStream,
    value: &T,
) -> std::io::Result<()> {
    let encoded = serde_json::to_string(value)
        .map_err(|err| std::io::Error::other(format!("failed to encode json: {err}")))?;
    write_text_line_allow_disconnect(stream, &encoded)
}

pub(in crate::server::api) fn should_stop_connection(
    stream: &mut LocalStream,
    running: &Arc<AtomicBool>,
) -> std::io::Result<bool> {
    if !running.load(Ordering::Relaxed) {
        return Ok(true);
    }

    local_stream_peer_closed(stream)
}

pub(in crate::server::api) fn dispatch_to_app_with_timeout(
    request: Request,
    api_tx: &ApiRequestSender,
    timeout: Option<Duration>,
) -> String {
    dispatch_to_app(request, api_tx, timeout, None)
}

pub(in crate::server::api) fn dispatch_to_app_with_caller_timeout(
    request: Request,
    api_tx: &ApiRequestSender,
    timeout: Option<Duration>,
) -> String {
    dispatch_to_app(
        request,
        api_tx,
        timeout,
        Some(("timeout", "timed out waiting for agent status")),
    )
}

fn dispatch_to_app(
    request: Request,
    api_tx: &ApiRequestSender,
    timeout: Option<Duration>,
    timeout_response: Option<(&str, &str)>,
) -> String {
    let request_id = request.id.clone();
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    if let Err(err) = api_tx.send(ApiRequestMessage {
        request,
        respond_to,
    }) {
        return error_response_json(
            request_id,
            "server_unavailable",
            format!("failed to dispatch request: {err}"),
        );
    }

    let response = match timeout {
        Some(timeout) => response_rx.recv_timeout(timeout).map_err(|err| match err {
            std::sync::mpsc::RecvTimeoutError::Timeout => std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "timed out waiting for app response after {} ms",
                    timeout.as_millis()
                ),
            ),
            std::sync::mpsc::RecvTimeoutError::Disconnected => std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "app response channel closed",
            ),
        }),
        None => response_rx
            .recv()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err)),
    };

    match response {
        Ok(response) => response,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::TimedOut {
                if let Some((code, message)) = timeout_response {
                    return error_response_json(request_id, code, message.into());
                }
            }
            error_response_json(
                request_id,
                "server_unavailable",
                format!("request handling failed: {err}"),
            )
        }
    }
}

pub(super) fn error_response_json(id: String, code: &str, message: String) -> String {
    serde_json::to_string(&ErrorResponse {
        id,
        error: ErrorBody {
            code: code.into(),
            message,
        },
    })
    .unwrap_or_else(|_| {
        r#"{"id":"","error":{"code":"internal_error","message":"failed to encode error response"}}"#
            .to_string()
    })
}
