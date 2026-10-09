//! Apply launch failures to the saved model without consulting terminal I/O.

use std::collections::HashSet;

use super::{Node, RestoredSession, TileLayout};
use crate::server::workspaces::{Tab, Workspace};
use crate::utils::ids::TerminalId;

impl RestoredSession {
    pub(in crate::server) fn discard_failed_launches(&mut self, failed: &HashSet<TerminalId>) {
        if failed.is_empty() {
            return;
        }
        self.terminals.retain(|id, _| !failed.contains(id));
        let workspaces = std::mem::take(&mut self.workspaces);
        let tab_number_bases = std::mem::take(&mut self.workspace_tab_number_bases);
        for (mut workspace, tab_number_base) in workspaces.into_iter().zip(tab_number_bases) {
            prune_workspace(&mut workspace, failed, tab_number_base);
            if !workspace.tabs.is_empty() {
                self.workspaces.push(workspace);
                self.workspace_tab_number_bases.push(tab_number_base);
            }
        }
    }
}

fn prune_workspace(
    workspace: &mut Workspace,
    failed: &HashSet<TerminalId>,
    tab_number_base: usize,
) {
    let active_number = workspace
        .tabs
        .get(workspace.active_tab)
        .map(|tab| tab.number);
    workspace.tabs.retain_mut(|tab| prune_tab(tab, failed));
    let surviving: HashSet<_> = workspace
        .tabs
        .iter()
        .flat_map(|tab| tab.layout.pane_ids())
        .collect();
    workspace
        .public_pane_numbers
        .retain(|id, _| surviving.contains(id));
    // Follow the saved active tab to its new index when earlier tabs were dropped;
    // only a dropped active tab falls back to the clamped old position.
    workspace.active_tab = workspace
        .tabs
        .iter()
        .position(|tab| Some(tab.number) == active_number)
        .unwrap_or_else(|| {
            workspace
                .active_tab
                .min(workspace.tabs.len().saturating_sub(1))
        });
    workspace.next_public_tab_number = workspace
        .tabs
        .iter()
        .fold(tab_number_base, |next, tab| next.max(tab.number + 1));
}

fn prune_tab(tab: &mut Tab, failed: &HashSet<TerminalId>) -> bool {
    tab.panes
        .retain(|_, pane| !failed.contains(&pane.attached_terminal_id));
    if tab.panes.is_empty() {
        tracing::warn!(tab = ?tab.custom_name, "no panes could be restored for tab, dropping it");
        return false;
    }
    let surviving = tab.panes.keys().copied().collect();
    let Some(node) = super::prune_restored_node(copy_node(tab.layout.root()), &surviving) else {
        tracing::warn!(tab = ?tab.custom_name,
            "restored tab lost all panes after pruning missing layout nodes");
        return false;
    };
    let pane_ids = super::collect_pane_ids(&node);
    let Some(&first) = pane_ids.first() else {
        return false;
    };
    let focus = tab.layout.focused();
    let focus = if surviving.contains(&focus) {
        focus
    } else {
        first
    };
    if !surviving.contains(&tab.root_pane) {
        tab.root_pane = first;
    }
    tab.layout = TileLayout::from_saved(node, focus);
    true
}

fn copy_node(node: &Node) -> Node {
    match node {
        Node::Pane(id) => Node::Pane(*id),
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => Node::Split {
            direction: *direction,
            ratio: *ratio,
            first: Box::new(copy_node(first)),
            second: Box::new(copy_node(second)),
        },
    }
}
