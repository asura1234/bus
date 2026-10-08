use super::*;

impl ClientShellState {
    pub(in crate::client) fn push_pane_mouse_event(
        &self,
        hit: &PaneHit,
        mouse: MouseEvent,
        modifiers: crossterm::event::KeyModifiers,
        outcome: &mut ClientShellInput,
    ) {
        let kind = crate::protocol::ClientMouseKind::from_crossterm(mouse.kind);
        let position = self.pane_mouse_position(hit, mouse);
        let geometry = matches!(position, ClientMousePosition::Pixels { .. }).then_some(
            crate::protocol::ClientMouseGeometry {
                cols: hit.inner_rect.width,
                rows: hit.inner_rect.height,
                width_px: hit.pixel_width,
                height_px: hit.pixel_height,
            },
        );
        let target = ClientInputTarget::Pane(hit.pane_id.clone());
        push_target_event(
            target,
            ClientPaneInputEvent::Mouse {
                kind,
                position,
                geometry,
                modifiers: modifiers.bits(),
                lines: self.config.mouse_scroll_lines.min(u16::MAX as usize) as u16,
            },
            outcome,
        );
    }
}
