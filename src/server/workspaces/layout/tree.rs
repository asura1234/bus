use super::Node;
use crate::utils::ids::PaneId;
use ratatui::layout::Direction;

pub(super) fn count_panes(node: &Node) -> usize {
    match node {
        Node::Pane(_) => 1,
        Node::Split { first, second, .. } => count_panes(first) + count_panes(second),
    }
}

pub(super) fn collect_ids(node: &Node, ids: &mut Vec<PaneId>) {
    match node {
        Node::Pane(id) => ids.push(*id),
        Node::Split { first, second, .. } => {
            collect_ids(first, ids);
            collect_ids(second, ids);
        }
    }
}

pub(super) fn split_ratios(node: &Node) -> Vec<(Vec<bool>, f32)> {
    fn collect(node: &Node, path: &mut Vec<bool>, out: &mut Vec<(Vec<bool>, f32)>) {
        match node {
            Node::Pane(_) => {}
            Node::Split {
                ratio,
                first,
                second,
                ..
            } => {
                out.push((path.clone(), *ratio));
                path.push(false);
                collect(first, path, out);
                path.pop();
                path.push(true);
                collect(second, path, out);
                path.pop();
            }
        }
    }

    let mut out = Vec::new();
    collect(node, &mut Vec::new(), &mut out);
    out
}

pub(super) fn swap_pane_ids(node: &mut Node, first: PaneId, second: PaneId) {
    match node {
        Node::Pane(id) if *id == first => *id = second,
        Node::Pane(id) if *id == second => *id = first,
        Node::Pane(_) => {}
        Node::Split {
            first: first_child,
            second: second_child,
            ..
        } => {
            swap_pane_ids(first_child, first, second);
            swap_pane_ids(second_child, first, second);
        }
    }
}

pub(super) fn split_at(
    node: Node,
    target: PaneId,
    direction: Direction,
    new_id: PaneId,
    split_ratio: f32,
) -> Node {
    match node {
        Node::Pane(id) if id == target => Node::Split {
            direction,
            ratio: split_ratio,
            first: Box::new(Node::Pane(id)),
            second: Box::new(Node::Pane(new_id)),
        },
        Node::Pane(_) => node,
        Node::Split {
            direction: d,
            ratio,
            first,
            second,
        } => Node::Split {
            direction: d,
            ratio,
            first: Box::new(split_at(*first, target, direction, new_id, split_ratio)),
            second: Box::new(split_at(*second, target, direction, new_id, split_ratio)),
        },
    }
}

pub(super) fn valid_split_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.clamp(0.1, 0.9)
    } else {
        0.5
    }
}

pub(super) fn remove_pane(node: Node, target: PaneId) -> Option<Node> {
    match node {
        Node::Pane(id) if id == target => None,
        Node::Pane(_) => Some(node),
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => match (remove_pane(*first, target), remove_pane(*second, target)) {
            (None, Some(s)) => Some(s),
            (Some(f), None) => Some(f),
            (Some(f), Some(s)) => Some(Node::Split {
                direction,
                ratio,
                first: Box::new(f),
                second: Box::new(s),
            }),
            (None, None) => None,
        },
    }
}

pub(super) fn set_ratio_at(node: &mut Node, path: &[bool], new_ratio: f32) -> bool {
    if let Node::Split {
        ratio,
        first,
        second,
        ..
    } = node
    {
        if path.is_empty() {
            *ratio = new_ratio;
            true
        } else if path[0] {
            set_ratio_at(second, &path[1..], new_ratio)
        } else {
            set_ratio_at(first, &path[1..], new_ratio)
        }
    } else {
        false
    }
}
