//! Composer delivery pill and its keyboard- and mouse-selectable options.
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

const FULL_PILL_WIDTH: u16 = 20;
const COMPACT_PILL_WIDTH: u16 = 13;

pub(super) fn delivery_pill_width(width: u16) -> u16 {
    if width >= FULL_PILL_WIDTH + 19 {
        FULL_PILL_WIDTH
    } else {
        // Leave space for +, @ and a recipient chip, including its border.
        width
            .saturating_sub(11)
            .clamp(1, COMPACT_PILL_WIDTH)
            .min(width)
    }
}

pub(super) fn delivery_pill(view: &mut View, rect: Rect, mode: SendMode, open: bool) {
    let label = match mode {
        Interrupt => "Interrupt",
        Steer => "Steer",
        Queue => "Queue",
    };
    let width = usize::from(rect.width);
    let text = if width + 1 == usize::from(FULL_PILL_WIDTH) {
        format!(" Mode: {label} ⌄ ")
    } else if width > label.len() + 1 {
        format!(" {label} ⌄")
    } else if width >= label.len() {
        label.to_owned()
    } else {
        format!("{}…", &label[..width.saturating_sub(1)])
    };
    // Paint padding too, so the text and hit target share one pill.
    view.overlay_row(rect, text, Some(Action::DeliveryMode), open);
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
        let options = OPTIONS
            .map(|(mode, label, detail)| (mode, wrap(&format!("{label}  {detail}"), width - 2)));
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
