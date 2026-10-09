//! Monotonic snapshot/surface pairing and foreground snapshot installation.
use super::{ClientInputLeases, ClientShellState, ShellHitMap};
use crate::client::{
    connection::{bootstrap as endpoint, write_to_server},
    effects::client_shell_resize_message,
    errors::ClientError,
    state::ClientState,
};
use crate::protocol::wire::{ClientShellSnapshot, PaneSurfaceFrame};

fn pane_surface_row<'a>(
    surface: &'a PaneSurfaceFrame,
    pane: &crate::protocol::wire::PaneSurfacePane,
    absolute_row: u32,
) -> Option<&'a [crate::protocol::wire::CellData]> {
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
    selection: &crate::utils::text::selection::Selection<String>,
    previous_surface: &PaneSurfaceFrame,
    previous_pane: &crate::protocol::wire::PaneSurfacePane,
    next_surface: &PaneSurfaceFrame,
    next_pane: &crate::protocol::wire::PaneSurfacePane,
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

impl ClientShellState {
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
}

pub(in crate::client) fn install_client_shell_snapshot(
    state: &mut ClientState,
    snapshot: Box<crate::protocol::wire::ClientShellSnapshot>,
    connection: &mut endpoint::ServerConnection,
) -> Result<(), ClientError> {
    let Some(shell) = state.shell.as_mut() else {
        return Ok(());
    };
    let previous_size = shell.surface_size(state.reported_size.0, state.reported_size.1);
    shell.install_snapshot(snapshot);
    let graphics_cleanup = shell.take_pending_graphics_cleanup();
    let next_size = shell.surface_size(state.reported_size.0, state.reported_size.1);
    let composed = shell.compose(state.reported_size.0, state.reported_size.1);
    let resize = (previous_size != next_size).then(|| {
        client_shell_resize_message(
            shell,
            state.reported_size.0,
            state.reported_size.1,
            state.reported_cell_size.0,
            state.reported_cell_size.1,
            state.pixel_geometry_exact,
        )
    });
    state.present_graphics(&graphics_cleanup);
    if let Some(resize) = resize {
        write_to_server(connection, &resize).map_err(ClientError::ConnectionLost)?;
    }
    if let Some(frame) = composed {
        state.present_frame(frame);
    }
    Ok(())
}
