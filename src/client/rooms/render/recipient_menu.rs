//! Composer recipient bar and its bounded picker.
use super::super::{recipients::Layout as RecipientLayout, BusUi, LocalRoom};
use super::delivery_mode::{delivery_pill, delivery_pill_width};
use super::sidebar::{message_name_color, status_name};
use super::text::provider;
use super::{Action, View, ACCENT};
use crate::messaging::model::Room;
use ratatui::layout::{Margin, Rect};

impl BusUi {
    pub(super) fn recipient_bar_view(
        &self,
        view: &mut View,
        local: &LocalRoom,
        recipients: &RecipientLayout,
    ) {
        let Rect {
            x,
            y: composer_y,
            width,
            height: bar_height,
        } = view.recipient_bar;
        let controls_y = composer_y + u16::from(bar_height > 1);
        let pill_width = delivery_pill_width(width);
        let pill = Rect::new(
            x,
            controls_y,
            pill_width.saturating_sub(1).max(1).min(pill_width),
            1,
        );
        delivery_pill(view, pill, local.delivery, self.delivery_menu.is_some());
        let x = x + pill_width;
        let width = width.saturating_sub(pill_width);
        view.row(
            Rect::new(x, controls_y, width.min(3), 1),
            "+",
            Some(Action::Files),
            false,
            false,
        );
        view.row(
            Rect::new(
                x + 3,
                controls_y,
                if local.recipients.is_empty() {
                    width.saturating_sub(3)
                } else {
                    width.saturating_sub(3).min(3)
                },
                1,
            ),
            if local.recipients.is_empty() {
                "@  Choose agents"
            } else {
                "@"
            },
            Some(Action::Recipients),
            false,
            true,
        );
        for chip in &recipients.chips {
            if chip.rect.y < self.recipient_scroll
                || chip.rect.bottom() > self.recipient_scroll + bar_height
            {
                continue;
            }
            let rect = Rect::new(
                x + chip.rect.x,
                composer_y + chip.rect.y - self.recipient_scroll,
                chip.rect.width,
                3,
            );
            let color = self
                .snapshot
                .state
                .agent(chip.agent)
                .map_or(ACCENT, |agent| {
                    message_name_color(&self.snapshot.state, self.room, agent, &self.settings)
                });
            view.recipient_chips.push((rect, color));
            let label_rect = Rect::new(rect.x + 2, rect.y + 1, rect.width.saturating_sub(4), 1);
            view.row(label_rect, &chip.label, None, false, false);
            view.color_last_row(label_rect, color);
            view.hit(rect, Action::Recipients);
        }
    }

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
        view.begin_picker(menu);
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
    }
}
