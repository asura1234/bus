use std::io;

use tracing::{debug, warn};

use crate::protocol::wire::NotifyKind;

pub(super) fn handle_notify(kind: NotifyKind, message: &str, body: Option<&str>) {
    handle_notify_with_notifiers(
        kind,
        message,
        body,
        super::host_terminal::notify::show_notification,
        crate::platform::show_desktop_notification,
    );
}

/// Writes a pane's terminal bells to the host terminal, except under Bus, whose
/// rooms' sound settings are the only source of its sounds.
pub(super) fn forward_terminal_bells(
    writer: &mut impl io::Write,
    count: u16,
    bus_shell: bool,
) -> io::Result<()> {
    if bus_shell {
        return Ok(());
    }
    super::host_terminal::effects::write_terminal_bells(writer, count)
}

pub(super) fn handle_notify_with_notifiers(
    kind: NotifyKind,
    message: &str,
    body: Option<&str>,
    mut show_terminal_notification: impl FnMut(&str, Option<&str>) -> io::Result<bool>,
    mut show_system_notification: impl FnMut(&str, Option<&str>) -> io::Result<bool>,
) {
    match kind {
        NotifyKind::Toast => {
            debug!(
                message = message,
                "received terminal toast notification from server"
            );
            if let Err(err) = show_terminal_notification(message, body) {
                warn!(err = %err, "failed to emit terminal notification");
            }
        }
        NotifyKind::SystemToast => {
            debug!(
                message = message,
                "received system toast notification from server"
            );
            if let Err(err) = show_system_notification(message, body) {
                warn!(err = %err, "failed to emit system notification");
            }
        }
    }
}
