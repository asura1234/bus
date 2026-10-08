//! Pane hit geometry and in-progress host mouse/chrome gestures.
use super::*;

#[derive(Default)]
pub(in crate::client) struct ShellHitMap {
    pub(in crate::client) panes: Vec<PaneHit>,
    pub(in crate::client) pane_splits: Vec<PaneSplitHit>,
    pub(in crate::client) notification_toast: Rect,
}

#[derive(Clone)]
pub(in crate::client) struct PaneHit {
    pub(in crate::client) rect: Rect,
    pub(in crate::client) inner_rect: Rect,
    pub(in crate::client) scrollbar_rect: Option<Rect>,
    pub(in crate::client) scroll: Option<crate::pane::ScrollMetrics>,
    pub(in crate::client) pane_id: String,
    pub(in crate::client) mouse_reporting: bool,
    pub(in crate::client) sgr_pixel_mouse: bool,
    pub(in crate::client) pixel_width: u32,
    pub(in crate::client) pixel_height: u32,
}

#[derive(Clone)]
pub(in crate::client) struct PaneSplitHit {
    pub(in crate::client) direction: crate::protocol::PaneSurfaceSplitDirection,
    pub(in crate::client) pos: u16,
    pub(in crate::client) area: Rect,
    pub(in crate::client) hit_rect: Rect,
    pub(in crate::client) path: Vec<bool>,
    pub(in crate::client) topology_signature: u64,
}

pub(in crate::client) struct ClientPaneMouseGesture {
    pub(in crate::client) hit: PaneHit,
    pub(in crate::client) button: crossterm::event::MouseButton,
    pub(in crate::client) stripped_modifiers: crossterm::event::KeyModifiers,
    pub(in crate::client) last_event: crossterm::event::MouseEvent,
    pub(in crate::client) last_position: crate::protocol::ClientMousePosition,
}

pub(in crate::client) enum ClientChromeDrag {
    PaneSplit {
        hit: PaneSplitHit,
        tab_id: String,
        grab_offset: i32,
        last_sent_ratio: Option<f32>,
        last_sent_at: Option<std::time::Instant>,
    },
    PaneScrollbar {
        hit: PaneHit,
        grab_row_offset: u16,
        last_sent_offset: Option<usize>,
        last_sent_at: Option<std::time::Instant>,
    },
}
