//! BSP tree layout for tiling panes within a workspace.

use ratatui::{
    layout::{Direction, Rect},
    widgets::Borders,
};

use crate::utils::ids::PaneId;

mod geometry;
mod nav;
mod tree;
use geometry::{collect_panes, collect_splits};
pub use nav::find_in_direction;
use nav::{nearest_resize_split, opposite_direction};
use tree::{
    collect_ids, count_panes, remove_pane, set_ratio_at, split_at, split_ratios, swap_pane_ids,
    valid_split_ratio,
};

/// Snapshot of a pane's position and focus state after layout.
#[derive(Clone)]
pub struct PaneInfo {
    pub id: PaneId,
    /// Outer rect (including borders if present).
    pub rect: Rect,
    /// Inner rect (content area, excluding borders). Used for selection.
    pub inner_rect: Rect,
    /// Visible scrollbar lane, when scrollback is present. `inner_rect` may still
    /// exclude a stable hidden gutter when this is `None`.
    pub scrollbar_rect: Option<Rect>,
    /// Borders drawn around this pane after UI chrome is applied.
    pub borders: Borders,
    pub is_focused: bool,
}

/// Info about a split boundary, used for mouse drag resize.
#[derive(Clone)]
pub struct SplitBorder {
    /// Position of the divider line (x for horizontal split, y for vertical).
    pub pos: u16,
    /// Direction of the split that created this border.
    pub direction: Direction,
    /// Ratio assigned to the first child of this split.
    pub ratio: f32,
    /// Total area of the split node.
    pub area: Rect,
    /// Path from root to this split node (false=first, true=second).
    pub path: Vec<bool>,
}

/// Cardinal direction for pane navigation.
#[derive(Debug, Clone, Copy)]
pub enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

/// A node in the BSP tree. Public for serialization.
pub enum Node {
    Pane(PaneId),
    Split {
        direction: Direction,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// BSP tiling layout. Tracks a tree of splits and a focused pane.
pub struct TileLayout {
    root: Node,
    focus: PaneId,
    /// Pane focused before `focus`, used by `close_focused`. Only a real focus
    /// move writes it; tree edits go through the target-taking primitives
    /// (`split_pane`, `close_pane`) so internal
    /// focus excursions never corrupt it.
    prev_focus: Option<PaneId>,
}

impl TileLayout {
    /// Create a new layout with a single pane (globally unique ID).
    /// Returns (layout, root_pane_id) so the caller can create the pane.
    pub fn new() -> (Self, PaneId) {
        let root_id = PaneId::alloc();
        (
            Self {
                root: Node::Pane(root_id),
                focus: root_id,
                prev_focus: None,
            },
            root_id,
        )
    }

    /// Move focus, recording the pane being left. No-op when focus is unchanged.
    fn set_focus(&mut self, id: PaneId) {
        if id != self.focus {
            self.prev_focus = Some(self.focus);
            self.focus = id;
        }
    }

    pub fn focused(&self) -> PaneId {
        self.focus
    }

    pub fn pane_count(&self) -> usize {
        count_panes(&self.root)
    }

    /// Compute rects for all panes given the available area.
    pub fn panes(&self, area: Rect) -> Vec<PaneInfo> {
        let mut result = Vec::new();
        collect_panes(&self.root, area, self.focus, &mut result);
        result
    }

    /// Collect all split boundaries for mouse drag resize.
    pub fn splits(&self, area: Rect) -> Vec<SplitBorder> {
        let mut result = Vec::new();
        collect_splits(&self.root, area, vec![], &mut result);
        result
    }

    /// Split the focused pane. Returns the new pane's id. Production splits
    /// flow through `Tab` so a failed runtime spawn can roll back; this remains
    /// as the user-split shape for tests.
    #[cfg(test)]
    pub fn split_focused(&mut self, direction: Direction) -> PaneId {
        self.split_focused_with_ratio(direction, 0.5)
    }

    /// Split the focused pane with a custom first-child ratio.
    #[cfg(test)]
    pub fn split_focused_with_ratio(&mut self, direction: Direction, ratio: f32) -> PaneId {
        let new_id = self
            .split_pane(self.focus, direction, ratio)
            .expect("focused pane is in the layout");
        self.set_focus(new_id);
        new_id
    }

    /// Split `target` without moving focus. Returns the new pane's id, or None
    /// when `target` is not in the layout.
    pub fn split_pane(
        &mut self,
        target: PaneId,
        direction: Direction,
        ratio: f32,
    ) -> Option<PaneId> {
        if !self.pane_ids().contains(&target) {
            return None;
        }
        let new_id = PaneId::alloc();
        let placeholder = PaneId::from_raw(0);
        let old = std::mem::replace(&mut self.root, Node::Pane(placeholder));
        self.root = split_at(old, target, direction, new_id, valid_split_ratio(ratio));
        Some(new_id)
    }

    /// Close the focused pane, returning focus to the pane it came from when
    /// that pane is still open. Returns false if it's the last pane.
    pub fn close_focused(&mut self) -> bool {
        if self.pane_count() <= 1 {
            return false;
        }
        let target = self.focus;
        let ids = self.pane_ids();
        // Focus is kept in the tree by construction and by every focus/layout update.
        // If that invariant is ever broken, leave the layout intact instead of panicking.
        let Some(pos) = ids.iter().position(|id| *id == target) else {
            return false;
        };
        let ordered = if pos + 1 < ids.len() {
            ids[pos + 1]
        } else {
            ids[pos - 1]
        };
        let new_focus = match self.prev_focus {
            Some(prev) if prev != target && ids.contains(&prev) => prev,
            _ => ordered,
        };
        let placeholder = PaneId::from_raw(0);
        let old = std::mem::replace(&mut self.root, Node::Pane(placeholder));
        if let Some(new_root) = remove_pane(old, target) {
            self.root = new_root;
            self.focus = new_focus;
            self.prev_focus = None;
            true
        } else {
            false
        }
    }

    /// Close any pane. Focus and its history are left alone unless the closed
    /// pane is the focused one.
    pub fn close_pane(&mut self, id: PaneId) -> bool {
        if self.focus == id {
            return self.close_focused();
        }
        if self.pane_count() <= 1 || !self.pane_ids().contains(&id) {
            return false;
        }
        let placeholder = PaneId::from_raw(0);
        let old = std::mem::replace(&mut self.root, Node::Pane(placeholder));
        let Some(new_root) = remove_pane(old, id) else {
            return false;
        };
        self.root = new_root;
        if self.prev_focus == Some(id) {
            self.prev_focus = None;
        }
        true
    }

    pub fn focus_pane(&mut self, id: PaneId) {
        if self.pane_ids().contains(&id) {
            self.set_focus(id);
        }
    }

    /// Swap two pane ids in the layout tree while preserving split shape and
    /// ratios. Returns true only when both panes exist and are different.
    pub fn swap_panes(&mut self, first: PaneId, second: PaneId) -> bool {
        if first == second {
            return false;
        }
        let ids = self.pane_ids();
        if !ids.contains(&first) || !ids.contains(&second) {
            return false;
        }
        swap_pane_ids(&mut self.root, first, second);
        true
    }

    /// Set the ratio of a split node at the given path.
    pub fn set_ratio_at(&mut self, path: &[bool], ratio: f32) -> bool {
        set_ratio_at(&mut self.root, path, ratio.clamp(0.1, 0.9))
    }

    /// Adjust the nearest split in the given direction for the focused pane.
    /// `delta` is positive to grow, negative to shrink.
    pub fn resize_focused(&mut self, nav: NavDirection, delta: f32, area: Rect) {
        let panes = self.panes(area);
        let Some(focused) = panes.iter().find(|p| p.is_focused) else {
            return;
        };
        let focused_rect = focused.rect;
        let splits = self.splits(area);

        let target_dir = match nav {
            NavDirection::Left | NavDirection::Right => Direction::Horizontal,
            NavDirection::Up | NavDirection::Down => Direction::Vertical,
        };
        let grows = matches!(nav, NavDirection::Right | NavDirection::Down);

        let best = nearest_resize_split(&splits, target_dir, focused_rect, nav).or_else(|| {
            nearest_resize_split(&splits, target_dir, focused_rect, opposite_direction(nav))
        });

        if let Some(split) = best {
            let adj = if grows { delta } else { -delta };
            self.set_ratio_at(&split.path, split.ratio + adj);
        }
    }

    pub fn resize_pane(
        &mut self,
        pane_id: PaneId,
        nav: NavDirection,
        delta: f32,
        area: Rect,
    ) -> bool {
        if !self.pane_ids().contains(&pane_id) {
            return false;
        }
        let before = split_ratios(&self.root);
        let previous_focus = self.focus;
        self.focus = pane_id;
        self.resize_focused(nav, delta, area);
        self.focus = previous_focus;
        split_ratios(&self.root) != before
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        collect_ids(&self.root, &mut ids);
        ids
    }

    /// Access the tree root for serialization.
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// Reconstruct a layout from a saved tree.
    pub fn from_saved(root: Node, focus: PaneId) -> Self {
        Self {
            root,
            focus,
            prev_focus: None,
        }
    }
}

#[cfg(test)]
#[path = "layout_test.rs"]
mod tests;
