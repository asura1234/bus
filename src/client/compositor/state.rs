use super::*;

// Sibling client components share this projection; keep access confined to client.

fn pane_surface_row<'a>(
    surface: &'a PaneSurfaceFrame,
    pane: &crate::protocol::PaneSurfacePane,
    absolute_row: u32,
) -> Option<&'a [crate::protocol::CellData]> {
    let viewport_top = pane
        .scroll
        .map(|scroll| {
            scroll
                .max_offset_from_bottom
                .saturating_sub(scroll.offset_from_bottom) as u32
        })
        .unwrap_or(0);
    let viewport_row = u16::try_from(absolute_row.checked_sub(viewport_top)?).ok()?;
    if viewport_row >= pane.inner_rect.height {
        return None;
    }
    let start = (usize::from(pane.inner_rect.y) + usize::from(viewport_row))
        * usize::from(surface.frame.width)
        + usize::from(pane.inner_rect.x);
    surface
        .frame
        .cells
        .get(start..start + usize::from(pane.inner_rect.width))
}

fn selection_cells_unchanged(
    selection: &crate::selection::Selection<String>,
    previous_surface: &PaneSurfaceFrame,
    previous_pane: &crate::protocol::PaneSurfacePane,
    next_surface: &PaneSurfaceFrame,
    next_pane: &crate::protocol::PaneSurfacePane,
) -> bool {
    let ((start_row, start_col), (end_row, end_col)) = selection.ordered_cells();
    (start_row..=end_row).all(|row| {
        let first_col = if row == start_row { start_col } else { 0 };
        let last_col = if row == end_row {
            end_col
        } else {
            previous_pane.inner_rect.width.saturating_sub(1)
        };
        pane_surface_row(previous_surface, previous_pane, row)
            .zip(pane_surface_row(next_surface, next_pane, row))
            .and_then(|(previous, next)| {
                previous
                    .get(usize::from(first_col)..=usize::from(last_col))
                    .zip(next.get(usize::from(first_col)..=usize::from(last_col)))
            })
            .is_some_and(|(previous, next)| {
                previous
                    .iter()
                    .zip(next)
                    .all(|(previous, next)| previous.symbol == next.symbol)
            })
    })
}

pub(crate) struct ClientShellConfig {
    pub(in crate::client) copy_on_select: bool,
    pub(in crate::client) clipboard_toast_enabled: bool,
    pub(in crate::client) clipboard_toast_position: crate::config::ToastClipboardPosition,
    pub(in crate::client) theme_runtime: crate::app::state::ThemeRuntimeConfig,
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

#[derive(Debug)]
pub(crate) enum ClientShellAction {
    Endpoint {
        boot_id: String,
        request: Box<crate::api::schema::Request>,
    },
    ClipboardWrite(Vec<u8>),
    EditComposer,
    OpenSafeWebUrl(String),
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

type ClientInputLeases = crate::input::InputLeaseTable<u8, ClientInputContext, ClientInputTarget>;

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
    pub(in crate::client) graphics: crate::kitty_graphics::surface::ClientState,
    pub(in crate::client) graphics_cell_size: crate::kitty_graphics::HostCellSize,
    pub(in crate::client) chrome_drag: Option<ClientChromeDrag>,
    pub(in crate::client) last_composed_size: Option<(u16, u16)>,
    pub(in crate::client) hits: ShellHitMap,
    pub(in crate::client) agent_presentation:
        crate::client::connection::agent_seen::EndpointAgentPresentation,
    pub(in crate::client) previous_pane_id: Option<String>,
    pub(in crate::client) pane_mouse_gesture: Option<ClientPaneMouseGesture>,
    pub(in crate::client) url_click_consumes_until_up: bool,
    pub(in crate::client) replaying_url_click: bool,
    pub(in crate::client) selection: Option<crate::selection::Selection<String>>,
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
    pub(in crate::client) copy_feedback: Option<crate::app::state::CopyFeedback>,
    pub(in crate::client) copy_feedback_deadline: Option<std::time::Instant>,
    pub(in crate::client) host_mouse_pixels: Option<crate::input::mouse::HostPixels>,
    pub(in crate::client) input_leases: ClientInputLeases,
    pub(in crate::client) next_request_id: u64,
    pub(in crate::client) pending_requests: HashMap<String, PendingEndpointRequest>,
    pub(in crate::client) outer_focused: Option<bool>,
    pub(in crate::client) host_appearance: Option<crate::terminal_theme::HostAppearance>,
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
            graphics: crate::kitty_graphics::surface::ClientState::default(),
            graphics_cell_size: crate::kitty_graphics::HostCellSize {
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

    pub(in crate::client) fn reset_endpoint_projection(&mut self) {
        self.hits = ShellHitMap::default();
        self.pane_surface = None;
        self.pending_pane_surface = None;
        self.input_leases = ClientInputLeases::default();
        self.chrome_drag = None;
        self.last_composed_size = None;
        self.pending_requests.clear();
        self.pane_scroll_in_flight.clear();
        self.pane_scroll_queued.clear();
        self.pane_scroll_targets.clear();
        self.previous_pane_id = None;
        self.pane_mouse_gesture = None;
        self.url_click_consumes_until_up = false;
        self.replaying_url_click = false;
        self.selection = None;
        self.last_pane_click = None;
        self.selection_autoscroll = None;
        self.selection_autoscroll_deadline = None;
        self.selection_highlight_clear_deadline = None;
        self.pending_word_selection = None;
        self.copy_feedback = None;
        self.copy_feedback_deadline = None;
        self.host_mouse_pixels = None;
    }

    pub(in crate::client) fn apply_active_snapshot(&mut self, snapshot: Box<ClientShellSnapshot>) {
        let graphics_scope = snapshot.boot_id.clone();
        let endpoint_boot_changed =
            self.snapshot.is_some() && self.graphics.scope() != graphics_scope;
        if !endpoint_boot_changed
            && self.snapshot.as_ref().is_some_and(|current| {
                current.boot_id == snapshot.boot_id && snapshot.revision < current.revision
            })
        {
            return;
        }
        self.graphics.set_scope(&graphics_scope);
        self.config_diagnostic = super::config::merged_config_diagnostic(
            self.local_config_diagnostic.as_deref(),
            snapshot.config_diagnostic.as_deref(),
        );
        let boot_changed = endpoint_boot_changed
            || self
                .snapshot
                .as_ref()
                .is_some_and(|current| current.boot_id != snapshot.boot_id);
        if boot_changed
            || self
                .pane_surface
                .as_ref()
                .is_none_or(|surface| surface.projection_revision != snapshot.revision)
        {
            self.hits = ShellHitMap::default();
        }
        if boot_changed {
            self.reset_endpoint_projection();
        } else if let Some(previous) = self
            .snapshot
            .as_deref()
            .and_then(|current| current.focused_pane_id.as_ref())
            .filter(|previous| Some(previous.as_str()) != snapshot.focused_pane_id.as_deref())
        {
            self.previous_pane_id = Some(previous.clone());
        }
        if self.selection.as_ref().is_some_and(|selection| {
            snapshot.focused_pane_id.as_deref() != Some(selection.pane_id.as_str())
                || !snapshot
                    .panes
                    .iter()
                    .any(|pane| pane.pane_id == selection.pane_id)
        }) {
            self.selection = None;
            self.selection_autoscroll = None;
            self.selection_autoscroll_deadline = None;
            self.selection_highlight_clear_deadline = None;
            self.pending_word_selection = None;
            self.last_pane_click = None;
        }
        let pane_exists =
            |pane_id: &String| snapshot.panes.iter().any(|pane| &pane.pane_id == pane_id);
        self.pane_scroll_in_flight
            .retain(|pane_id, _| pane_exists(pane_id));
        self.pane_scroll_queued
            .retain(|pane_id, _| pane_exists(pane_id));
        self.pane_scroll_targets
            .retain(|pane_id, _| pane_exists(pane_id));

        self.snapshot = Some(snapshot);
        let pending_surface = self.pending_pane_surface.take();
        if let Some(surface) = pending_surface {
            let matching = self.snapshot.as_ref().is_some_and(|snapshot| {
                surface.boot_id == snapshot.boot_id
                    && surface.projection_revision == snapshot.revision
            });
            if matching {
                self.install_pane_surface(surface, false);
            } else if self.snapshot.as_ref().is_some_and(|snapshot| {
                surface.boot_id == snapshot.boot_id
                    && surface.projection_revision > snapshot.revision
            }) {
                self.pending_pane_surface = Some(surface);
            }
        }
    }

    pub(crate) fn set_pane_surface(&mut self, surface: PaneSurfaceFrame) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        if surface.boot_id != snapshot.boot_id || surface.projection_revision < snapshot.revision {
            return;
        }
        if self.pane_surface.as_ref().is_some_and(|current| {
            current.boot_id == surface.boot_id
                && (surface.projection_revision < current.projection_revision
                    || (surface.projection_revision == current.projection_revision
                        && surface.surface_revision < current.surface_revision))
        }) {
            return;
        }
        if surface.projection_revision == snapshot.revision.saturating_add(1) {
            // The next expected surface waits separately for its exact snapshot. Keeping the
            // current pair avoids treating this speculative successor as presentation evidence.
            self.pending_pane_surface = Some(surface);
            self.hits = ShellHitMap::default();
            return;
        }
        // A surface that skips one or more revisions supersedes any retained pair, but is still
        // not rendered until its matching snapshot arrives. Retain it monotonically so delayed
        // intermediate surfaces cannot replace it.
        self.install_pane_surface(surface, true);
    }

    fn install_pane_surface(&mut self, mut surface: PaneSurfaceFrame, retain_future: bool) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        if surface.boot_id != snapshot.boot_id
            || surface.projection_revision < snapshot.revision
            || (!retain_future && surface.projection_revision != snapshot.revision)
            || self.pane_surface.as_ref().is_some_and(|current| {
                current.boot_id == surface.boot_id
                    && (surface.projection_revision < current.projection_revision
                        || (surface.projection_revision == current.projection_revision
                            && surface.surface_revision < current.surface_revision))
            })
        {
            return;
        }
        // A retained future surface is not presentable yet. Clear hit targets immediately; the
        // exact-pair compose guard prevents it from replacing the visible frame.
        if surface.projection_revision != snapshot.revision {
            self.hits = ShellHitMap::default();
        }
        self.acknowledge_active_surface_agents(&surface);
        let selection_content_changed = self.selection.as_ref().is_some_and(|selection| {
            let Some(previous_surface) = self.pane_surface.as_ref() else {
                return false;
            };
            let previous = previous_surface
                .panes
                .iter()
                .find(|pane| pane.pane_id == selection.pane_id);
            let next = surface
                .panes
                .iter()
                .find(|pane| pane.pane_id == selection.pane_id);
            let (Some(previous), Some(next)) = (previous, next) else {
                return false;
            };
            previous.inner_rect.width != next.inner_rect.width
                || previous.inner_rect.height != next.inner_rect.height
                || previous.alternate_screen_active != next.alternate_screen_active
                // Manual mouse selections track a live buffer range, not a content revision.
                || (self.config.copy_on_select
                && previous.content_revision != next.content_revision
                && (!previous.content_revision.is_multiple_of(2)
                    || !next.content_revision.is_multiple_of(2)
                    || !selection_cells_unchanged(
                        selection,
                        previous_surface,
                        previous,
                        &surface,
                        next,
                    )))
        });
        if selection_content_changed {
            self.selection = None;
            self.stop_selection_autoscroll();
            self.selection_highlight_clear_deadline = None;
        }
        for pane in &surface.panes {
            let Some(target) = self.pane_scroll_targets.get(&pane.pane_id).copied() else {
                continue;
            };
            let Some(scroll) = pane.scroll else {
                continue;
            };
            let target =
                target.min(usize::try_from(scroll.max_offset_from_bottom).unwrap_or(usize::MAX));
            if usize::try_from(scroll.offset_from_bottom).unwrap_or(usize::MAX) == target {
                self.pane_scroll_targets.remove(&pane.pane_id);
            }
        }
        self.graphics
            .set_scene(std::mem::take(&mut surface.graphics));
        self.pane_surface = Some(surface);
    }

    pub(crate) fn show_copy_feedback(&mut self, now: std::time::Instant) -> bool {
        if !self.config.clipboard_toast_enabled {
            return false;
        }
        self.copy_feedback = Some(crate::app::state::CopyFeedback {
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
        let default = std::time::Duration::from_millis(100);
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
