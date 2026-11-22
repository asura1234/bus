//! Composer delivery arrow and its keyboard- and mouse-selectable options.
use super::{wrap, Action, View, ACCENT};
use crate::messaging::model::SendMode::{self, Interrupt, Queue, Steer};
use ratatui::{
    layout::{Margin, Rect},
    style::{Color, Style},
};

const OPTIONS: [(SendMode, &str, &str); 3] = [
    (
        Interrupt,
        "Interrupt",
        "Stop the running turn and deliver now.",
    ),
    (Steer, "Steer", "Join the running turn when eligible."),
    (Queue, "Queue", "Deliver after the current turn ends."),
];

/// ▼ plus the blank column the toolbar leaves before `+`.
const TRIGGER_RESERVE: u16 = 2;
const TRIGGER: &str = "▼";

pub(super) fn delivery_pill_width(width: u16) -> u16 {
    TRIGGER_RESERVE.min(width)
}

pub(super) fn delivery_pill(view: &mut View, rect: Rect, _mode: SendMode, open: bool) {
    // The current mode is labeled only in the open menu. The closed hit is the arrow.
    view.overlay_row(rect, TRIGGER.to_owned(), Some(Action::DeliveryMode), open);
    view.style_last_row(
        rect,
        Style::default().fg(ACCENT).bg(if open {
            Color::Rgb(48, 58, 55)
        } else {
            Color::Rgb(36, 43, 42)
        }),
    );
}

impl View {
    pub(super) fn delivery_menu(&mut self, selected: SendMode) {
        let composer = self.composer_box;
        let width = composer.width.min(52);
        if width < 3 || composer.bottom() < 3 {
            return;
        }
        let options = OPTIONS.map(|(mode, label, detail)| {
            let mark = if mode == selected { "● " } else { "  " };
            (mode, wrap(&format!("{mark}{label}  {detail}"), width - 2))
        });
        let desired = 2 + options
            .iter()
            .map(|(_, lines)| lines.len() as u16)
            .sum::<u16>();
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
        for (mode, lines) in options {
            if body.height < 3 && mode != selected {
                continue;
            }
            let height = if menu.height == desired {
                lines.len() as u16
            } else {
                1
            };
            for (index, line) in lines.into_iter().take(usize::from(height)).enumerate() {
                self.overlay_row(
                    Rect::new(body.x, y + index as u16, body.width, 1),
                    line,
                    None,
                    mode == selected,
                );
            }
            self.hit(
                Rect::new(body.x, y, body.width, height),
                Action::ChooseDelivery(mode),
            );
            y += height;
        }
    }
}
