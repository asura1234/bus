//! Request-method classes that decide shell focus reconciliation and geometry claims.

use crate::protocol::api;
use crate::server::main_loop::HeadlessServer;

impl HeadlessServer {
    pub(super) fn shell_locations_may_need_reconcile(method: &api::schema::Method) -> bool {
        use crate::protocol::api::schema::Method;

        matches!(
            method,
            Method::PaneClose(_)
                | Method::PaneCloseIfIdentity(_)
                | Method::PaneSplit(_)
                | Method::TabClose(_)
                | Method::TabCreate(_)
                | Method::WorkspaceClose(_)
                | Method::WorkspaceCreate(_)
        )
    }

    pub(super) fn shell_endpoint_claims_geometry(method: &api::schema::Method) -> bool {
        use crate::protocol::api::schema::Method;

        matches!(
            method,
            Method::LayoutSetSplitRatio(_)
                | Method::PaneClose(_)
                | Method::PaneCloseIfIdentity(_)
                | Method::PaneCopyMotion(_)
                | Method::PaneCopySearch(_)
                | Method::PaneFocus(_)
                | Method::PaneFocusDirection(_)
                | Method::PaneInputSet(_)
                | Method::PaneLinkActivate(_)
                | Method::PaneRename(_)
                | Method::PaneResize(_)
                | Method::PaneScroll(_)
                | Method::PaneSplit(_)
                | Method::PaneSwap(_)
                | Method::PaneZoom(_)
                | Method::TabClose(_)
                | Method::TabCreate(_)
                | Method::TabFocus(_)
                | Method::TabRename(_)
                | Method::WorkspaceClose(_)
                | Method::WorkspaceCreate(_)
                | Method::WorkspaceFocus(_)
                | Method::WorkspaceMove(_)
                | Method::WorkspaceRename(_)
        )
    }

    pub(super) fn public_request_may_change_geometry(method: &api::schema::Method) -> bool {
        use crate::protocol::api::schema::Method;

        matches!(
            method,
            Method::LayoutSetSplitRatio(_)
                | Method::PaneClose(_)
                | Method::PaneCloseIfIdentity(_)
                | Method::PaneFocus(_)
                | Method::PaneFocusDirection(_)
                | Method::PaneResize(_)
                | Method::PaneSplit(_)
                | Method::PaneSwap(_)
                | Method::PaneZoom(_)
                | Method::TabClose(_)
                | Method::TabCreate(_)
                | Method::TabFocus(_)
                | Method::WorkspaceClose(_)
                | Method::WorkspaceCreate(_)
                | Method::WorkspaceFocus(_)
        )
    }
}
