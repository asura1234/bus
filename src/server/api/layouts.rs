use ratatui::layout::Direction;

use crate::protocol::api::schema::{
    LayoutDescription, LayoutNode, LayoutPane, LayoutSetSplitRatioParams, ResponseResult,
    SplitDirection,
};
use crate::server::app::App;
use crate::{server::workspaces::layout::Node, utils::ids::PaneId};

use super::errors::{encode_error, encode_success};

impl App {
    pub(super) fn handle_layout_set_split_ratio(
        &mut self,
        id: String,
        params: LayoutSetSplitRatioParams,
    ) -> String {
        if !params.ratio.is_finite() {
            return encode_error(id, "invalid_ratio", "ratio must be finite");
        }
        let Some((ws_idx, tab_idx)) =
            self.resolve_layout_target(params.tab_id.as_deref(), params.pane_id.as_deref())
        else {
            return encode_error(id, "layout_not_found", "layout target not found");
        };

        let changed = self
            .state
            .workspaces
            .get_mut(ws_idx)
            .and_then(|ws| ws.tabs.get_mut(tab_idx))
            .is_some_and(|tab| tab.layout.set_ratio_at(&params.path, params.ratio));
        if !changed {
            return encode_error(id, "split_not_found", "split path not found");
        }

        self.schedule_session_save();
        let Some(layout) = self.layout_description(ws_idx, tab_idx) else {
            return encode_error(id, "layout_not_found", "layout unavailable");
        };
        self.emit_layout_updated_event(ws_idx, tab_idx);
        encode_success(id, ResponseResult::LayoutSplitRatioSet { layout })
    }

    fn resolve_layout_target(
        &self,
        tab_id: Option<&str>,
        pane_id: Option<&str>,
    ) -> Option<(usize, usize)> {
        match (tab_id, pane_id) {
            (Some(_), Some(_)) => None,
            (Some(tab_id), None) => self.parse_tab_id(tab_id),
            (None, Some(pane_id)) => {
                let (ws_idx, pane_id) = self.parse_pane_id(pane_id)?;
                let tab_idx = self
                    .state
                    .workspaces
                    .get(ws_idx)?
                    .find_tab_index_for_pane(pane_id)?;
                Some((ws_idx, tab_idx))
            }
            (None, None) => {
                let ws_idx = self.state.active?;
                let tab_idx = self.state.workspaces.get(ws_idx)?.active_tab_index();
                Some((ws_idx, tab_idx))
            }
        }
    }

    fn layout_description(&self, ws_idx: usize, tab_idx: usize) -> Option<LayoutDescription> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab = ws.tabs.get(tab_idx)?;
        Some(LayoutDescription {
            workspace_id: self.public_workspace_id(ws_idx),
            tab_id: self.public_tab_id(ws_idx, tab_idx)?,
            zoomed: tab.zoomed,
            focused_pane_id: self.public_pane_id(ws_idx, tab.layout.focused())?,
            root: self.layout_node_description(ws_idx, tab_idx, tab.layout.root())?,
        })
    }

    fn layout_node_description(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        node: &Node,
    ) -> Option<LayoutNode> {
        match node {
            Node::Pane(pane_id) => Some(LayoutNode::Pane {
                pane: self.layout_pane_description(ws_idx, tab_idx, *pane_id)?,
            }),
            Node::Split {
                direction,
                ratio,
                first,
                second,
            } => Some(LayoutNode::Split {
                direction: match direction {
                    Direction::Horizontal => SplitDirection::Right,
                    Direction::Vertical => SplitDirection::Down,
                },
                ratio: *ratio,
                first: Box::new(self.layout_node_description(ws_idx, tab_idx, first)?),
                second: Box::new(self.layout_node_description(ws_idx, tab_idx, second)?),
            }),
        }
    }

    fn layout_pane_description(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        pane_id: PaneId,
    ) -> Option<LayoutPane> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab = ws.tabs.get(tab_idx)?;
        let terminal_id = tab.terminal_id(pane_id)?;
        let terminal = self.state.terminals.get(terminal_id);
        Some(LayoutPane {
            pane_id: Some(self.public_pane_id(ws_idx, pane_id)?),
            label: terminal.and_then(|terminal| terminal.manual_label.clone()),
            cwd: tab
                .cwd_for_pane(pane_id, &self.state.terminals, &self.terminal_runtimes)
                .map(|cwd| cwd.display().to_string()),
            command: terminal.and_then(|terminal| terminal.launch_argv.clone()),
            env: Default::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::exiting_test_command;
    use super::*;
    use crate::{
        protocol::api::schema::{ErrorResponse, EventData, ResponseResult, SuccessResponse},
        server::workspaces::Workspace,
        utils::config::{Config, ShellModeConfig},
    };

    fn app_with_workspace() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::server::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::server::api::EventHub::default(),
        );
        app.state.default_shell = exiting_test_command().into();
        app.state.shell_mode = ShellModeConfig::NonLogin;
        app.state.workspaces = vec![Workspace::test_new("layout")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.ensure_test_terminals();
        app
    }

    #[test]
    fn layout_set_split_ratio_preserves_portable_tree() {
        let mut app = app_with_workspace();
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let right = app.state.workspaces[0].test_split(Direction::Horizontal);
        app.state.ensure_test_terminals();
        app.state.workspaces[0].tabs[0].layout.focus_pane(root);
        app.state.workspaces[0].tabs[0]
            .layout
            .set_ratio_at(&[], 0.65);
        let right_terminal_id = app.state.workspaces[0].tabs[0]
            .terminal_id(right)
            .cloned()
            .unwrap();
        app.state
            .terminals
            .get_mut(&right_terminal_id)
            .unwrap()
            .set_manual_label("tests".into());

        let response = app.handle_layout_set_split_ratio(
            "req".into(),
            LayoutSetSplitRatioParams {
                tab_id: None,
                pane_id: None,
                path: Vec::new(),
                ratio: 0.65,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::LayoutSplitRatioSet { layout } = success.result else {
            panic!("expected split ratio response");
        };
        assert_eq!(layout.workspace_id, app.public_workspace_id(0));
        assert_eq!(layout.focused_pane_id, app.public_pane_id(0, root).unwrap());
        let LayoutNode::Split {
            direction,
            ratio,
            second,
            ..
        } = layout.root
        else {
            panic!("expected split layout root");
        };
        assert_eq!(direction, SplitDirection::Right);
        assert!((ratio - 0.65).abs() < f32::EPSILON);
        let LayoutNode::Pane { pane } = *second else {
            panic!("expected second pane");
        };
        assert_eq!(pane.label.as_deref(), Some("tests"));
        assert_eq!(pane.pane_id, Some(app.public_pane_id(0, right).unwrap()));
    }

    #[test]
    fn layout_set_split_ratio_updates_existing_split() {
        let mut app = app_with_workspace();
        app.state.workspaces[0].test_split(Direction::Horizontal);

        let response = app.handle_layout_set_split_ratio(
            "req".into(),
            LayoutSetSplitRatioParams {
                tab_id: None,
                pane_id: None,
                path: vec![],
                ratio: 0.72,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::LayoutSplitRatioSet { layout } = success.result else {
            panic!("expected layout split ratio set response");
        };
        let LayoutNode::Split { ratio, .. } = layout.root else {
            panic!("expected split layout root");
        };
        assert!((ratio - 0.72).abs() < f32::EPSILON);
        assert!(matches!(
            &app.event_hub.events_after(0).last().expect("layout event").1.data,
            EventData::LayoutUpdated { layout }
                if layout.tab_id == app.public_tab_id(0, 0).unwrap()
                    && (layout.splits[0].ratio - 0.72).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn layout_set_split_ratio_targets_a_pane_in_an_inactive_workspace() {
        let mut app = app_with_workspace();
        let mut other = Workspace::test_new("other");
        other.test_split(Direction::Horizontal);
        let pane = other.tabs[0].root_pane;
        app.state.workspaces.push(other);
        app.state.ensure_test_terminals();

        let response = app.handle_layout_set_split_ratio(
            "req".into(),
            LayoutSetSplitRatioParams {
                tab_id: None,
                pane_id: app.public_pane_id(1, pane),
                path: vec![],
                ratio: 0.72,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::LayoutSplitRatioSet { layout } = success.result else {
            panic!("expected split ratio response");
        };
        assert_eq!(layout.workspace_id, app.public_workspace_id(1));
        assert_eq!(app.state.active, Some(0));
        let Node::Split { ratio, .. } = app.state.workspaces[1].tabs[0].layout.root() else {
            panic!("expected split layout root");
        };
        assert!((*ratio - 0.72).abs() < f32::EPSILON);
    }

    #[test]
    fn layout_set_split_ratio_rejects_missing_split() {
        let mut app = app_with_workspace();

        let response = app.handle_layout_set_split_ratio(
            "req".into(),
            LayoutSetSplitRatioParams {
                tab_id: None,
                pane_id: None,
                path: vec![],
                ratio: 0.72,
            },
        );

        let error: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "split_not_found");
    }
}
