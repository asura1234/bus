//! A one-button error dialog that holds the keyboard and mouse until dismissed.
use super::{render::Action, BusUi};
use crate::protocol::keys::host::RawInputEvent;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};

pub(super) struct Alert {
    pub title: String,
    pub message: String,
}

impl BusUi {
    pub(super) fn show_alert(&mut self, title: String, message: String) {
        self.alert = Some(Alert { title, message });
        self.last_click = None;
    }

    pub(super) fn dismiss_alert(&mut self) {
        self.alert = None;
        self.last_click = None;
    }

    pub(super) fn alert_input(&mut self, event: &RawInputEvent) -> bool {
        if self.alert.is_none() {
            return false;
        }
        match event {
            RawInputEvent::Key(key) => {
                if key.kind == KeyEventKind::Press
                    && key.modifiers == KeyModifiers::NONE
                    && matches!(key.code, KeyCode::Enter | KeyCode::Esc)
                {
                    self.dismiss_alert();
                }
            }
            RawInputEvent::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && self.view.hits.iter().rev().any(|hit| {
                        hit.action == Action::DismissAlert
                            && hit.rect.contains((mouse.column, mouse.row).into())
                    })
                {
                    self.dismiss_alert();
                }
            }
            RawInputEvent::Paste(_) | RawInputEvent::Text(_) => {}
            _ => return false,
        }
        true
    }
}
