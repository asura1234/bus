//! Composer delivery arrow and its keyboard- and mouse-selectable options.
use super::{Action, View, ACCENT};
use crate::messaging::model::SendMode::{self, Interrupt, Queue, Steer};
use ratatui::{
    layout::{Margin, Rect},
    style::{Color, Style},
};

const OPTIONS: [(SendMode, &str); 3] =
    [(Interrupt, "Interrupt"), (Steer, "Steer"), (Queue, "Queue")];
/// The border, the selection marker and a space, and the longest label.
const MENU_WIDTH: u16 = 2 + 2 + "Interrupt".len() as u16;

/// ▼ plus two blank columns, matching the gap between `+` and `@`.
const TRIGGER_RESERVE: u16 = 3;
const TRIGGER: &str = "▼";

pub(super) fn delivery_pill_width(width: u16) -> u16 {
    TRIGGER_RESERVE.min(width)
}

pub(super) fn delivery_pill(view: &mut View, rect: Rect, open: bool) {
    view.row(rect, TRIGGER, Some(Action::DeliveryMode), open, false);
    if open {
        view.style_last_row(rect, Style::default().fg(ACCENT).bg(Color::Rgb(48, 58, 55)));
    }
}

impl View {
    pub(super) fn delivery_menu(&mut self, current: SendMode, focused: SendMode) {
        let composer = self.composer_box;
        let width = composer.width.min(MENU_WIDTH);
        if width < 3 || composer.bottom() < 3 {
            return;
        }
        let desired = 2 + OPTIONS.len() as u16;
        let bottom = if composer.y >= desired {
            composer.y
        } else {
            composer.bottom()
        };
        let menu = Rect::new(
            composer.x,
            bottom.saturating_sub(desired),
            width,
            desired.min(bottom),
        );
        let body = menu.inner(Margin::new(1, 1));
        self.begin_picker(menu);
        let mut y = body.y;
        for (mode, label) in OPTIONS {
            if body.height < OPTIONS.len() as u16 && mode != focused {
                continue;
            }
            let mark = if mode == current { "● " } else { "  " };
            self.overlay_row(
                Rect::new(body.x, y, body.width, 1),
                format!("{mark}{label}"),
                Some(Action::ChooseDelivery(mode)),
                mode == focused,
            );
            y += 1;
        }
    }
}
