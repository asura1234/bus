use std::collections::HashMap;

mod agent_preview;
mod compose;
mod config;
mod hits;
pub(super) mod patch;
mod render;
pub(super) mod snapshot;

pub(in crate::client) use hits::*;
#[cfg(test)]
pub(super) use patch::apply_composed_surface_patch;
pub(super) use patch::{ClientComposedSurfacePatch, ClientPaneSurfacePatchOutcome};

#[cfg(test)]
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

#[cfg(test)]
use crate::protocol::keys::host::RawInputEvent;
#[cfg(test)]
use crate::protocol::wire::ClientMousePosition;
use crate::protocol::wire::{
    ClientMessage, ClientPaneInputEvent, ClientShellSnapshot, ClientSurfaceSize, FrameData,
    PaneSurfaceFrame,
};
use crate::utils::config::Config;
use crate::utils::theme::Palette;
#[cfg(test)]
use crossterm::event::KeyCode;

// Former shell descendants now live in sibling client components and share these helpers.
pub(super) fn push_target_event(
    target: ClientInputTarget,
    event: ClientPaneInputEvent,
    outcome: &mut ClientShellInput,
) {
    let ClientInputTarget::Pane(pane_id) = target;
    if let Some(ClientMessage::ClientShellPaneInput {
        pane_id: pending_pane,
        events,
    }) = outcome.requests.last_mut()
    {
        if *pending_pane == pane_id {
            events.push(event);
            return;
        }
    }
    outcome.requests.push(ClientMessage::ClientShellPaneInput {
        pane_id,
        events: vec![event],
    });
}

pub(super) fn contains(rect: Rect, point: (u16, u16)) -> bool {
    rect.width > 0
        && rect.height > 0
        && point.0 >= rect.x
        && point.0 < rect.right()
        && point.1 >= rect.y
        && point.1 < rect.bottom()
}

pub(super) fn pane_surface_topology_signature(surface: &PaneSurfaceFrame) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn write(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(PRIME);
        }
        *hash ^= 0xff;
        *hash = hash.wrapping_mul(PRIME);
    }

    let mut pane_ids = surface
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_bytes())
        .collect::<Vec<_>>();
    pane_ids.sort_unstable();
    let mut hash = OFFSET;
    for pane_id in pane_ids {
        write(&mut hash, pane_id);
    }
    let mut splits = surface.splits.iter().collect::<Vec<_>>();
    splits.sort_by(|left, right| left.path.cmp(&right.path));
    for split in splits {
        write(
            &mut hash,
            &[match split.direction {
                crate::protocol::wire::PaneSurfaceSplitDirection::Horizontal => 0,
                crate::protocol::wire::PaneSurfaceSplitDirection::Vertical => 1,
            }],
        );
        write(
            &mut hash,
            &split
                .path
                .iter()
                .map(|right| u8::from(*right))
                .collect::<Vec<_>>(),
        );
    }
    hash
}

pub(super) fn status_priority(status: crate::protocol::api::schema::AgentStatus) -> u8 {
    use crate::protocol::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Done => 3,
        AgentStatus::Working => 2,
        AgentStatus::Idle => 1,
        AgentStatus::Unknown => 0,
    }
}

fn blit_pane_surface(target: &mut FrameData, source: &FrameData, area: Rect) {
    let copy_width = source.width.min(area.width);
    let copy_height = source.height.min(area.height);
    let hyperlink_base = target.hyperlinks.len() as u32;
    target.hyperlinks.extend(source.hyperlinks.iter().cloned());

    for row in 0..copy_height {
        for col in 0..copy_width {
            let source_index = row as usize * source.width as usize + col as usize;
            let target_x = area.x + col;
            let target_y = area.y + row;
            let target_index = target_y as usize * target.width as usize + target_x as usize;
            let (Some(source_cell), Some(target_cell)) = (
                source.cells.get(source_index),
                target.cells.get_mut(target_index),
            ) else {
                continue;
            };
            *target_cell = source_cell.clone();
            target_cell.hyperlink = source_cell.hyperlink.and_then(|index| {
                ((index as usize) < source.hyperlinks.len()).then_some(hyperlink_base + index)
            });
        }
    }

    target.cursor = source.cursor.as_ref().and_then(|cursor| {
        (cursor.x < copy_width && cursor.y < copy_height).then(|| {
            crate::protocol::wire::CursorState {
                x: area.x + cursor.x,
                y: area.y + cursor.y,
                visible: cursor.visible,
                shape: cursor.shape,
            }
        })
    });
    target.graphics.clear();
}

// Sibling client components share this projection; keep access confined to client.

pub(crate) struct ClientShellConfig {
    pub(in crate::client) copy_on_select: bool,
    pub(in crate::client) clipboard_toast_enabled: bool,
    pub(in crate::client) clipboard_toast_position: crate::utils::config::ToastClipboardPosition,
    pub(in crate::client) theme_runtime: crate::utils::theme::ThemeRuntimeConfig,
    pub(in crate::client) palette: Palette,
    pub(in crate::client) mouse_capture: bool,
    pub(in crate::client) mouse_scroll_lines: usize,
    pub(in crate::client) right_click_passthrough_modifiers: Option<crossterm::event::KeyModifiers>,
    pub(in crate::client) redraw_on_focus_gained: bool,
    pub(in crate::client) startup_config_diagnostic: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::client) struct ClientShellLayout {
    pub sidebar: Rect,
    pub pane_surface: Rect,
}

#[derive(Debug)]
pub(crate) enum ClientShellAction {
    Endpoint {
        boot_id: String,
        request: Box<crate::protocol::api::schema::Request>,
    },
    ClipboardWrite(Vec<u8>),
    EditComposer,
    OpenSafeWebUrl(String),
    /// A local file or directory, opened in its default app.
    OpenPath(std::path::PathBuf),
    ReplayMouse(Vec<crossterm::event::MouseEvent>),
}

#[derive(Default)]
pub(crate) struct ClientShellInput {
    pub detach: bool,
    pub repaint: bool,
    pub resize: bool,
    pub query_host_appearance: bool,
    pub query_host_theme: bool,
    pub requests: Vec<ClientMessage>,
    pub actions: Vec<ClientShellAction>,
}

#[derive(Debug)]
pub(in crate::client) enum PendingEndpointKind {
    BusTerminalFocus {
        pane_id: String,
        navigation: u64,
    },
    Generic,
    SelectionCopy,
    PaneScroll {
        pane_id: String,
        serial: u64,
    },
    WordSelection {
        pane_id: String,
        absolute_row: u32,
        col: u16,
        generation: u64,
    },
    PaneLinkActivate {
        pane_id: String,
        inner_rect: Rect,
        fallback_events: Vec<crossterm::event::MouseEvent>,
    },
}

pub(in crate::client) struct PendingEndpointRequest {
    pub(in crate::client) boot_id: String,
    pub(in crate::client) method_name: String,
    pub(in crate::client) kind: PendingEndpointKind,
}

pub(crate) struct ClientShellEndpointError {
    pub code: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::client) enum ClientInputTarget {
    Pane(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::client) struct ClientInputContext {
    pub(in crate::client) retained_selection: bool,
}

type ClientInputLeases =
    crate::client::panes::input_lease::InputLeaseTable<u8, ClientInputContext, ClientInputTarget>;

#[derive(Clone, Debug)]
pub(in crate::client) struct ClientPaneClick {
    pub(in crate::client) pane_id: String,
    pub(in crate::client) viewport_row: u16,
    pub(in crate::client) col: u16,
    pub(in crate::client) at: std::time::Instant,
}

impl ClientPaneClick {
    pub(in crate::client) fn is_double_click_for(&self, next: &Self) -> bool {
        self.pane_id == next.pane_id
            && next.at.duration_since(self.at) <= std::time::Duration::from_millis(350)
            && self.viewport_row.abs_diff(next.viewport_row) <= 1
            && self.col.abs_diff(next.col) <= 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::client) enum ClientSelectionAutoscrollDirection {
    Up,
    Down,
}

#[derive(Clone, Debug)]
pub(in crate::client) struct ClientSelectionAutoscroll {
    pub(in crate::client) pane_id: String,
    pub(in crate::client) direction: ClientSelectionAutoscrollDirection,
    pub(in crate::client) last_mouse_column: u16,
    pub(in crate::client) last_mouse_row: u16,
    pub(in crate::client) inner_rect: Rect,
    pub(in crate::client) offset_from_bottom: usize,
    pub(in crate::client) max_offset_from_bottom: usize,
}

pub(crate) struct ClientShellState {
    pub(in crate::client) bus: Option<crate::client::rooms::BusUi>,
    pub(in crate::client) config: ClientShellConfig,
    pub(in crate::client) snapshot: Option<Box<ClientShellSnapshot>>,
    pub(in crate::client) pane_surface: Option<PaneSurfaceFrame>,
    /// A future projection surface waits here until its matching snapshot arrives. The visible
    /// pane surface always remains an exact snapshot pair.
    pub(in crate::client) pending_pane_surface: Option<PaneSurfaceFrame>,
    /// Last terminal surface per tab, used only for presentation during agent navigation.
    bus_terminal_surfaces: HashMap<String, PaneSurfaceFrame>,
    presented_bus_terminal: Option<String>,
    pub(in crate::client) graphics: crate::client::host_terminal::kitty::scene::ClientState,
    pub(in crate::client) graphics_cell_size: crate::protocol::kitty::placement::HostCellSize,
    pub(in crate::client) chrome_drag: Option<ClientChromeDrag>,
    pub(in crate::client) last_composed_size: Option<(u16, u16)>,
    pub(in crate::client) hits: ShellHitMap,
    pub(in crate::client) agent_presentation:
        crate::client::connection::agent_seen::EndpointAgentPresentation,
    pub(in crate::client) previous_pane_id: Option<String>,
    pub(in crate::client) pane_mouse_gesture: Option<ClientPaneMouseGesture>,
    pub(in crate::client) url_click_consumes_until_up: bool,
    pub(in crate::client) replaying_url_click: bool,
    pub(in crate::client) selection: Option<crate::utils::text::selection::Selection<String>>,
    pub(in crate::client) last_pane_click: Option<ClientPaneClick>,
    pub(in crate::client) selection_autoscroll: Option<ClientSelectionAutoscroll>,
    pub(in crate::client) selection_autoscroll_deadline: Option<std::time::Instant>,
    pub(in crate::client) selection_highlight_clear_deadline: Option<std::time::Instant>,
    pub(in crate::client) pending_word_selection: Option<u64>,
    pub(in crate::client) word_selection_generation: u64,
    pub(in crate::client) next_scroll_serial: u64,
    pub(in crate::client) pane_scroll_in_flight: HashMap<String, u64>,
    pub(in crate::client) pane_scroll_queued: HashMap<String, usize>,
    pub(in crate::client) pane_scroll_targets: HashMap<String, usize>,
    pub(in crate::client) copy_feedback: Option<crate::utils::render::widgets::CopyFeedback>,
    pub(in crate::client) copy_feedback_deadline: Option<std::time::Instant>,
    pub(in crate::client) host_mouse_pixels: Option<crate::protocol::keys::mouse::HostPixels>,
    pub(in crate::client) input_leases: ClientInputLeases,
    pub(in crate::client) next_request_id: u64,
    pub(in crate::client) pending_requests: HashMap<String, PendingEndpointRequest>,
    pub(in crate::client) outer_focused: Option<bool>,
    pub(in crate::client) host_appearance: Option<crate::utils::theme::color::HostAppearance>,
    pub(in crate::client) host_appearance_explicit: bool,
    pub(in crate::client) local_config_diagnostic: Option<String>,
    pub(in crate::client) config_diagnostic: Option<String>,
}

impl ClientShellState {
    pub(crate) fn new(mut config: ClientShellConfig) -> Self {
        let local_config_diagnostic = config.startup_config_diagnostic.take();
        Self {
            bus: None,
            config,
            snapshot: None,
            pane_surface: None,
            pending_pane_surface: None,
            bus_terminal_surfaces: HashMap::new(),
            presented_bus_terminal: None,
            graphics: crate::client::host_terminal::kitty::scene::ClientState::default(),
            graphics_cell_size: crate::protocol::kitty::placement::HostCellSize {
                width_px: 1,
                height_px: 1,
            },
            chrome_drag: None,
            last_composed_size: None,
            hits: ShellHitMap::default(),
            agent_presentation:
                crate::client::connection::agent_seen::EndpointAgentPresentation::default(),
            previous_pane_id: None,
            pane_mouse_gesture: None,
            url_click_consumes_until_up: false,
            replaying_url_click: false,
            selection: None,
            last_pane_click: None,
            selection_autoscroll: None,
            selection_autoscroll_deadline: None,
            selection_highlight_clear_deadline: None,
            pending_word_selection: None,
            word_selection_generation: 0,
            next_scroll_serial: 0,
            pane_scroll_in_flight: HashMap::new(),
            pane_scroll_queued: HashMap::new(),
            pane_scroll_targets: HashMap::new(),
            copy_feedback: None,
            copy_feedback_deadline: None,
            host_mouse_pixels: None,
            input_leases: ClientInputLeases::default(),
            next_request_id: 1,
            pending_requests: HashMap::new(),
            outer_focused: None,
            host_appearance: None,
            host_appearance_explicit: false,
            config_diagnostic: local_config_diagnostic.clone(),
            local_config_diagnostic,
        }
    }

    pub(in crate::client) fn layout(&self, cols: u16, rows: u16) -> ClientShellLayout {
        if self.bus.is_some() {
            return crate::client::rooms::layout(cols, rows);
        }
        // Without Bus (the hidden test client) the pane surface fills the terminal.
        ClientShellLayout {
            sidebar: Rect::default(),
            pane_surface: Rect::new(0, 0, cols, rows),
        }
    }

    pub(crate) fn surface_size(&self, cols: u16, rows: u16) -> ClientSurfaceSize {
        let surface = self.layout(cols, rows).pane_surface;
        ClientSurfaceSize {
            cols: surface.width.max(1),
            rows: surface.height.max(1),
        }
    }

    pub(crate) fn show_copy_feedback(&mut self, now: std::time::Instant) -> bool {
        if !self.config.clipboard_toast_enabled {
            return false;
        }
        self.copy_feedback = Some(crate::utils::render::widgets::CopyFeedback {
            message: "copied to clipboard".to_owned(),
        });
        self.copy_feedback_deadline = Some(now + std::time::Duration::from_secs(2));
        true
    }

    pub(crate) fn tick_copy_feedback(&mut self, now: std::time::Instant) -> bool {
        let mut repaint = false;
        if self
            .copy_feedback_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.copy_feedback = None;
            self.copy_feedback_deadline = None;
            repaint = true;
        }
        if self
            .selection_highlight_clear_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.selection = None;
            self.selection_highlight_clear_deadline = None;
            repaint = true;
        }
        repaint
    }

    pub(crate) fn timer_delay(&self, now: std::time::Instant) -> std::time::Duration {
        let default = self.bus.as_ref().map_or(
            std::time::Duration::from_millis(100),
            crate::client::rooms::BusUi::tick_interval,
        );
        self.selection_autoscroll_deadline
            .map(|deadline| deadline.saturating_duration_since(now).min(default))
            .unwrap_or(default)
    }

    pub(crate) fn invalidate_pane_surface(&mut self) {
        self.pane_surface = None;
        self.pending_pane_surface = None;
        self.hits = ShellHitMap::default();
        self.host_mouse_pixels = None;
    }
}

impl ClientShellState {
    pub(crate) fn take_pending_graphics_cleanup(&mut self) -> Vec<u8> {
        self.graphics.take_pending_cleanup()
    }

    pub(crate) fn set_graphics_cell_size(&mut self, width_px: u32, height_px: u32) {
        self.graphics_cell_size = crate::protocol::kitty::placement::HostCellSize {
            width_px: width_px.max(1),
            height_px: height_px.max(1),
        };
    }

    fn compose_graphics(&mut self, frame: &mut FrameData, layout: ClientShellLayout) {
        let local_cover = self.config_diagnostic.is_some()
            || self.copy_feedback.is_some()
            || self
                .selection
                .as_ref()
                .is_some_and(|selection| selection.is_visible());
        let visibility = if local_cover {
            crate::protocol::wire::SurfaceGraphicsVisibility::Hidden
        } else {
            crate::protocol::wire::SurfaceGraphicsVisibility::Main
        };
        frame.graphics = self.graphics.encode(
            visibility,
            (layout.pane_surface.x, layout.pane_surface.y),
            self.graphics_cell_size,
        );
    }
}

#[cfg(test)]
#[path = "tests/compositor_test.rs"]
pub(super) mod tests;
