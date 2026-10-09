use super::{pane_surface_topology_signature, ClientShellState, PaneSplitHit};

impl ClientShellState {
    pub(super) fn pane_split_target_is_current(
        &self,
        hit: &PaneSplitHit,
        tab_id: &str,
    ) -> Option<bool> {
        let snapshot = self.snapshot.as_deref()?;
        let surface = self.pane_surface.as_ref()?;
        if snapshot.revision != surface.projection_revision {
            return None;
        }
        Some(
            snapshot.focused_tab_id.as_deref() == Some(tab_id)
                && pane_surface_topology_signature(surface) == hit.topology_signature,
        )
    }

    pub(super) fn pane_split_ratio(hit: &PaneSplitHit, grab_offset: i32, point: (u16, u16)) -> f32 {
        let (pointer, origin, length) = match hit.direction {
            crate::protocol::wire::PaneSurfaceSplitDirection::Horizontal => {
                (i32::from(point.0), i32::from(hit.area.x), hit.area.width)
            }
            crate::protocol::wire::PaneSurfaceSplitDirection::Vertical => {
                (i32::from(point.1), i32::from(hit.area.y), hit.area.height)
            }
        };
        ((pointer + grab_offset - origin) as f32 / f32::from(length.max(1))).clamp(0.1, 0.9)
    }
}
