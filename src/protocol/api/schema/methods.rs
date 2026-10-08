use super::{
    AgentDialogAnswerParams, AgentDialogChooseParams, AgentPromptIfIdleParams,
    AgentPromptIfUnboundParams, AgentPromptParams, AgentReadParams, AgentRenameParams,
    AgentSendKeysParams, AgentStartParams, AgentTarget, AgentWaitParams,
    ClientShellSurfaceSetParams, ClientWindowTitleSetParams, EmptyParams, EventsSubscribeParams,
    EventsWaitParams, LayoutSetSplitRatioParams, NotificationShowParams, PaneCloseIfIdentityParams,
    PaneCopyMotionParams, PaneCopySearchParams, PaneCurrentParams, PaneFocusDirectionParams,
    PaneInputSetParams, PaneLayoutParams, PaneLinkActivateParams, PaneListParams, PaneReadParams,
    PaneRenameParams, PaneReportAgentSessionParams, PaneResizeParams, PaneScrollParams,
    PaneSelectionReadParams, PaneSendInputParams, PaneSendKeysParams, PaneSendTextParams,
    PaneSplitParams, PaneSwapParams, PaneTarget, PaneWaitForOutputParams, PaneZoomParams,
    PingParams, TabCreateParams, TabListParams, TabRenameParams, TabTarget, WorkspaceCloseParams,
    WorkspaceCreateParams, WorkspaceMoveParams, WorkspaceRenameParams, WorkspaceTarget,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Request {
    pub id: String,
    #[serde(flatten)]
    pub method: Method,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "method", content = "params")]
// Request enums are short-lived wire values; keeping variants direct preserves
// the simple serde shape and avoids boxing churn across every caller.
#[allow(clippy::large_enum_variant)]
pub enum Method {
    #[serde(rename = "ping")]
    Ping(PingParams),
    #[serde(rename = "server.stop")]
    ServerStop(EmptyParams),
    #[serde(rename = "server.reload_config")]
    ServerReloadConfig(EmptyParams),
    #[serde(rename = "notification.show")]
    NotificationShow(NotificationShowParams),
    #[serde(rename = "client.window_title.set")]
    ClientWindowTitleSet(ClientWindowTitleSetParams),
    #[serde(rename = "client.window_title.clear")]
    ClientWindowTitleClear(EmptyParams),
    #[serde(rename = "client_shell.surface.set")]
    ClientShellSurfaceSet(ClientShellSurfaceSetParams),
    #[serde(rename = "session.snapshot")]
    SessionSnapshot(EmptyParams),
    #[serde(rename = "workspace.create")]
    WorkspaceCreate(WorkspaceCreateParams),
    #[serde(rename = "workspace.list")]
    WorkspaceList(EmptyParams),
    #[serde(rename = "workspace.get")]
    WorkspaceGet(WorkspaceTarget),
    #[serde(rename = "workspace.focus")]
    WorkspaceFocus(WorkspaceTarget),
    #[serde(rename = "workspace.rename")]
    WorkspaceRename(WorkspaceRenameParams),
    #[serde(rename = "workspace.move")]
    WorkspaceMove(WorkspaceMoveParams),
    #[serde(rename = "workspace.close")]
    WorkspaceClose(WorkspaceCloseParams),
    #[serde(rename = "tab.create")]
    TabCreate(TabCreateParams),
    #[serde(rename = "tab.list")]
    TabList(TabListParams),
    #[serde(rename = "tab.get")]
    TabGet(TabTarget),
    #[serde(rename = "tab.focus")]
    TabFocus(TabTarget),
    #[serde(rename = "tab.rename")]
    TabRename(TabRenameParams),
    #[serde(rename = "tab.close")]
    TabClose(TabTarget),
    #[serde(rename = "agent.list")]
    AgentList(EmptyParams),
    #[serde(rename = "agent.get")]
    AgentGet(AgentTarget),
    #[serde(rename = "agent.read")]
    AgentRead(AgentReadParams),
    #[serde(rename = "agent.dialog.observe")]
    AgentDialogObserve(AgentTarget),
    #[serde(rename = "agent.dialog.choose")]
    AgentDialogChoose(AgentDialogChooseParams),
    #[serde(rename = "agent.dialog.answer")]
    AgentDialogAnswer(AgentDialogAnswerParams),
    #[serde(rename = "agent.send_keys")]
    AgentSendKeys(AgentSendKeysParams),
    #[serde(rename = "agent.rename")]
    AgentRename(AgentRenameParams),
    #[serde(rename = "agent.focus")]
    AgentFocus(AgentTarget),
    #[serde(rename = "agent.start")]
    AgentStart(AgentStartParams),
    #[serde(rename = "agent.prompt")]
    AgentPrompt(AgentPromptParams),
    #[serde(rename = "agent.prompt_if_idle")]
    AgentPromptIfIdle(AgentPromptIfIdleParams),
    #[serde(rename = "agent.prompt_if_unbound")]
    AgentPromptIfUnbound(AgentPromptIfUnboundParams),
    #[serde(rename = "agent.wait")]
    AgentWait(AgentWaitParams),
    #[serde(rename = "pane.split")]
    PaneSplit(PaneSplitParams),
    #[serde(rename = "pane.swap")]
    PaneSwap(PaneSwapParams),
    #[serde(rename = "pane.zoom")]
    PaneZoom(PaneZoomParams),
    #[serde(rename = "pane.layout")]
    PaneLayout(PaneLayoutParams),
    #[serde(rename = "layout.set_split_ratio")]
    LayoutSetSplitRatio(LayoutSetSplitRatioParams),
    #[serde(rename = "pane.focus_direction")]
    PaneFocusDirection(PaneFocusDirectionParams),
    #[serde(rename = "pane.resize")]
    PaneResize(PaneResizeParams),
    #[serde(rename = "pane.scroll")]
    PaneScroll(PaneScrollParams),
    #[serde(rename = "pane.selection.read")]
    PaneSelectionRead(PaneSelectionReadParams),
    #[serde(rename = "pane.copy_motion")]
    PaneCopyMotion(PaneCopyMotionParams),
    #[serde(rename = "pane.copy_search")]
    PaneCopySearch(PaneCopySearchParams),
    #[serde(rename = "pane.list")]
    PaneList(PaneListParams),
    #[serde(rename = "pane.current")]
    PaneCurrent(PaneCurrentParams),
    #[serde(rename = "pane.get")]
    PaneGet(PaneTarget),
    #[serde(rename = "pane.focus")]
    PaneFocus(PaneTarget),
    #[serde(rename = "pane.input.set")]
    PaneInputSet(PaneInputSetParams),
    #[serde(rename = "pane.link.activate")]
    PaneLinkActivate(PaneLinkActivateParams),
    #[serde(rename = "pane.rename")]
    PaneRename(PaneRenameParams),
    #[serde(rename = "pane.send_text")]
    PaneSendText(PaneSendTextParams),
    #[serde(rename = "pane.send_keys")]
    PaneSendKeys(PaneSendKeysParams),
    #[serde(rename = "pane.send_input")]
    PaneSendInput(PaneSendInputParams),
    #[serde(rename = "pane.read")]
    PaneRead(PaneReadParams),
    #[serde(rename = "pane.report_agent_session")]
    PaneReportAgentSession(PaneReportAgentSessionParams),
    #[serde(rename = "pane.close")]
    PaneClose(PaneTarget),
    #[serde(rename = "events.subscribe")]
    EventsSubscribe(EventsSubscribeParams),
    #[serde(rename = "events.wait")]
    EventsWait(EventsWaitParams),
    #[serde(rename = "pane.wait_for_output")]
    PaneWaitForOutput(PaneWaitForOutputParams),
    /// Close only an exact owned terminal/session, or acknowledge that it is absent.
    #[serde(rename = "pane.close_if_identity")]
    PaneCloseIfIdentity(PaneCloseIfIdentityParams),
}

/// Stable wire labels shared by API logging and endpoint bookkeeping.
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
