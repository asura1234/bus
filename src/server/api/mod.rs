pub(crate) mod socket;
pub(crate) mod streams;

pub use socket::ServerHandle;
pub(crate) use socket::{api_method_name, start_server_with_stop_control};
pub use streams::event_hub::EventHub;

use tokio::sync::mpsc;

use crate::protocol::api::schema::{Method, Request};

pub(crate) fn request_changes_ui(request: &Request) -> bool {
    matches!(
        &request.method,
        Method::ServerReloadConfig(_)
            | Method::NotificationShow(_)
            | Method::WorkspaceCreate(_)
            | Method::WorkspaceFocus(_)
            | Method::WorkspaceRename(_)
            | Method::WorkspaceMove(_)
            | Method::WorkspaceClose(_)
            | Method::TabCreate(_)
            | Method::TabFocus(_)
            | Method::TabRename(_)
            | Method::TabClose(_)
            | Method::LayoutSetSplitRatio(_)
            | Method::AgentRename(_)
            | Method::AgentFocus(_)
            | Method::AgentStart(_)
            | Method::AgentPrompt(_)
            | Method::AgentPromptIfIdle(_)
            | Method::AgentPromptIfUnbound(_)
            | Method::AgentDialogChoose(_)
            | Method::AgentDialogAnswer(_)
            | Method::AgentSendKeys(_)
            | Method::PaneSplit(_)
            | Method::PaneSwap(_)
            | Method::PaneZoom(_)
            | Method::PaneFocusDirection(_)
            | Method::PaneResize(_)
            | Method::PaneScroll(_)
            | Method::PaneFocus(_)
            | Method::PaneInputSet(_)
            | Method::PaneRename(_)
            | Method::PaneReportAgentSession(_)
            | Method::PaneClose(_)
            | Method::PaneCloseIfIdentity(_)
    )
}

pub struct ApiRequestMessage {
    pub request: Request,
    pub respond_to: std::sync::mpsc::Sender<String>,
}

pub type ApiRequestSender = mpsc::UnboundedSender<ApiRequestMessage>;

mod agents;
mod env;
pub(in crate::server) mod errors;
pub(crate) mod events;
pub(crate) mod input_encoding;
mod layouts;
mod panes;
mod server_methods;
mod session;
mod tabs;
pub(crate) mod terminal_read;
mod workspaces;

#[cfg(test)]
use crate::events::AppEvent;
use crate::server::app::App;
#[cfg(test)]
use crate::server::app::{Mode, OverlayPaneState, ToastKind};
#[cfg(all(test, windows))]
use crate::server::terminals::respawn::RuntimeExitAction;

impl App {
    #[cfg(test)]
    pub(crate) fn handle_api_request(
        &mut self,
        request: crate::protocol::api::schema::Request,
    ) -> String {
        self.drain_all_internal_events();
        self.handle_api_request_after_internal_events_drained(request)
    }

    pub(crate) fn handle_api_request_after_internal_events_drained(
        &mut self,
        request: crate::protocol::api::schema::Request,
    ) -> String {
        self.sync_pending_terminal_titles();
        use crate::protocol::api::schema::{Method, ResponseResult, SuccessResponse};

        let response = match request.method {
            Method::ServerStop(_) => {
                self.state.should_quit = true;
                SuccessResponse {
                    id: request.id,
                    result: ResponseResult::Ok {},
                }
            }
            Method::ServerReloadConfig(_) => {
                let report = self.reload_config();
                SuccessResponse {
                    id: request.id,
                    result: ResponseResult::ConfigReload {
                        status: report.status,
                        diagnostics: report.diagnostics,
                    },
                }
            }
            Method::NotificationShow(params) => {
                return self.handle_notification_show(request.id, params);
            }
            Method::ClientWindowTitleSet(_) | Method::ClientWindowTitleClear(_) => {
                return errors::encode_success(
                    request.id,
                    ResponseResult::ClientWindowTitle {
                        changed: false,
                        reason: crate::protocol::api::schema::ClientWindowTitleReason::NoForegroundClient,
                    },
                );
            }
            Method::SessionSnapshot(_) => return self.handle_session_snapshot(request.id),
            Method::WorkspaceList(_) => return self.handle_workspace_list(request.id),
            Method::WorkspaceGet(target) => return self.handle_workspace_get(request.id, target),
            Method::WorkspaceCreate(params) => {
                return self.handle_workspace_create(request.id, params);
            }
            Method::WorkspaceFocus(target) => {
                return self.handle_workspace_focus(request.id, target)
            }
            Method::WorkspaceRename(params) => {
                return self.handle_workspace_rename(request.id, params);
            }
            Method::WorkspaceMove(params) => {
                return self.handle_workspace_move(request.id, params);
            }
            Method::WorkspaceClose(target) => {
                return self.handle_workspace_close(request.id, target)
            }
            Method::TabList(params) => return self.handle_tab_list(request.id, params),
            Method::TabGet(target) => return self.handle_tab_get(request.id, target),
            Method::TabCreate(params) => return self.handle_tab_create(request.id, params),
            Method::TabFocus(target) => return self.handle_tab_focus(request.id, target),
            Method::TabRename(params) => return self.handle_tab_rename(request.id, params),
            Method::TabClose(target) => return self.handle_tab_close(request.id, target),
            Method::AgentList(_) => return self.handle_agent_list(request.id),
            Method::AgentGet(target) => return self.handle_agent_get(request.id, target),
            Method::AgentFocus(target) => return self.handle_agent_focus(request.id, target),
            Method::AgentRename(params) => return self.handle_agent_rename(request.id, params),
            Method::AgentStart(params) => return self.handle_agent_start(request.id, params),
            Method::AgentPrompt(_)
            | Method::AgentPromptIfIdle(_)
            | Method::AgentPromptIfUnbound(_) => {
                return errors::encode_error(
                    request.id,
                    "invalid_request",
                    "agent.prompt is handled asynchronously by the app runtime",
                );
            }
            Method::AgentWait(_) => {
                return errors::encode_error(
                    request.id,
                    "invalid_request",
                    "agent.wait is handled by the api server",
                );
            }
            Method::AgentRead(params) => return self.handle_agent_read(request.id, params),
            Method::AgentDialogObserve(target) => {
                return self.handle_agent_dialog_observe(request.id, target)
            }
            Method::AgentDialogChoose(params) => {
                return self.handle_agent_dialog_choose(request.id, params)
            }
            Method::AgentDialogAnswer(params) => {
                return self.handle_agent_dialog_answer(request.id, params)
            }
            Method::AgentSendKeys(params) => {
                return self.handle_agent_send_keys(request.id, params)
            }
            Method::PaneSplit(params) => return self.handle_pane_split(request.id, params),
            Method::PaneSwap(params) => return self.handle_pane_swap(request.id, params),
            Method::PaneZoom(params) => return self.handle_pane_zoom(request.id, params),
            Method::PaneLayout(params) => return self.handle_pane_layout(request.id, params),
            Method::LayoutSetSplitRatio(params) => {
                return self.handle_layout_set_split_ratio(request.id, params);
            }
            Method::PaneFocusDirection(params) => {
                return self.handle_pane_focus_direction(request.id, params);
            }
            Method::PaneResize(params) => return self.handle_pane_resize(request.id, params),
            Method::PaneScroll(params) => return self.handle_pane_scroll(request.id, params),
            Method::PaneSelectionRead(params) => {
                return self.handle_pane_selection_read(request.id, params);
            }
            Method::PaneCopyMotion(params) => {
                return self.handle_pane_copy_motion(request.id, params);
            }
            Method::PaneCopySearch(params) => {
                return self.handle_pane_copy_search(request.id, params);
            }
            Method::PaneList(params) => return self.handle_pane_list(request.id, params),
            Method::PaneCurrent(params) => return self.handle_pane_current(request.id, params),
            Method::PaneGet(target) => return self.handle_pane_get(request.id, target),
            Method::PaneFocus(target) => return self.handle_pane_focus(request.id, target),
            Method::PaneInputSet(params) => return self.handle_pane_input_set(request.id, params),
            Method::PaneLinkActivate(params) => {
                return self.handle_pane_link_activate(request.id, params);
            }
            Method::PaneRename(params) => return self.handle_pane_rename(request.id, params),
            Method::PaneRead(params) => return self.handle_pane_read(request.id, params),
            Method::PaneReportAgentSession(params) => {
                return self.handle_pane_report_agent_session(request.id, params);
            }
            Method::PaneSendText(params) => return self.handle_pane_send_text(request.id, params),
            Method::PaneSendInput(params) => {
                return self.handle_pane_send_input(request.id, params)
            }
            Method::PaneClose(target) => return self.handle_pane_close(request.id, target),
            Method::PaneCloseIfIdentity(params) => {
                return self.handle_pane_close_if_identity(request.id, params)
            }
            Method::PaneSendKeys(params) => return self.handle_pane_send_keys(request.id, params),
            _ => {
                return errors::encode_error(
                    request.id,
                    "not_implemented",
                    "method not implemented yet",
                );
            }
        };

        serde_json::to_string(&response).unwrap()
    }
}
#[cfg(test)]
pub(super) mod test_support {
    pub(crate) fn exiting_test_command() -> &'static str {
        #[cfg(windows)]
        {
            "C:\\Windows\\System32\\whoami.exe"
        }
        #[cfg(not(windows))]
        {
            "/usr/bin/true"
        }
    }

    pub(crate) fn shutdown_test_runtimes(app: &mut crate::server::app::App) {
        let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
        for (_terminal_id, runtime) in runtimes {
            runtime.shutdown();
        }
    }
}

#[cfg(test)]
#[path = "tests/dispatch_test.rs"]
mod tests;
