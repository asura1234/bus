//! A bounded recipient viewport, painted above history and draft content.
use super::super::{BusUi, LocalRoom};
use super::text::provider;
use super::{sidebar::status_name, Action, View};
use crate::messaging::model::Room;
use ratatui::layout::{Margin, Rect};

impl BusUi {
    pub(super) fn clamp_recipient_index(&mut self) {
        // The highlight is shared by room views; a room switch or roster
        // shrink must never leave the viewport starting beyond its entries.
        if let Some(room) = self.room {
            self.recipient_index = self
                .recipient_index
                .min(self.snapshot.state.room_recipients(room).count());
        }
    }

    pub(super) fn recipient_menu_view(&self, view: &mut View, room: &Room, local: &LocalRoom) {
        if !self.recipient_menu {
            return;
        }
        let agents: Vec<_> = self.snapshot.state.room_recipients(room.id).collect();
        let entries = agents.len().saturating_add(1);
        let above = view.composer_box.y.saturating_sub(1);
        let wanted = entries.saturating_add(2);
        // Prefer the space above the composer. An expanded draft or a short
        // terminal instead lends the picker the main area's full height.
        let (bottom, available) = if usize::from(above) >= wanted.min(5) {
            (view.composer_box.y, above)
        } else {
            (view.composer_box.bottom(), view.composer_box.bottom())
        };
        let height = wanted.min(usize::from(available)) as u16;
        let menu = Rect::new(
            view.composer_box.x,
            bottom.saturating_sub(height),
            view.composer_box.width,
            height,
        );
        let body = menu.inner(Margin::new(1, 1));
        if body.is_empty() {
            return;
        }
        let selected = self.recipient_index.min(agents.len());
        let start = selected
            .saturating_sub(usize::from(body.height.saturating_sub(1)))
            .min(entries.saturating_sub(usize::from(body.height)));
        let members: Vec<_> = agents.iter().filter(|a| a.room_id == room.id).collect();
        let all = !members.is_empty() && members.iter().all(|a| local.recipients.contains(&a.id));
        let labels = std::iter::once((format!("[{}] All", if all { "x" } else { " " }), None))
            .chain(agents.iter().map(|agent| {
                (
                    format!(
                        "[{}] {}  {}  {}",
                        if local.recipients.contains(&agent.id) {
                            "x"
                        } else {
                            " "
                        },
                        agent.name,
                        provider(agent.provider),
                        status_name(agent.shown_status())
                    ),
                    Some(agent.id),
                )
            }));
        view.recipient_menu = menu;
        view.recipient_rows_start = view.rows.len();
        // Covered links/quote buttons cannot remain active through the panel.
        view.hits.retain(|hit| !hit.rect.intersects(menu));
        for (index, (label, id)) in labels
            .enumerate()
            .skip(start)
            .take(usize::from(body.height))
        {
            view.overlay_row(
                Rect::new(body.x, body.y + (index - start) as u16, body.width, 1),
                label,
                Some(Action::Recipient(id)),
                index == selected,
            );
        }
        view.cursor = None;
        view.selection.retain(|rect| !rect.intersects(menu));
        // Kitty graphics sit above text, so remove covered thumbnails too.
        view.thumbnails
            .retain(|thumbnail| !thumbnail.rect().intersects(menu));
    }
}
