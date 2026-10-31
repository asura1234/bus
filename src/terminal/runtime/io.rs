use crate::terminal::pty::actor::{InputCompletion, InputScope, PtyIoActorHandle};
use crate::terminal::runtime::detection_policy::mark_detection_content_changed;
use crate::terminal::runtime::TerminalRuntime;
use crate::terminal::runtime::WheelRouting;
use bytes::Bytes;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
#[cfg(test)]
use tokio::sync::watch;
use tracing::warn;

#[derive(Clone)]
pub(super) enum PaneRuntimeIo {
    Actor(PtyIoActorHandle),
    #[cfg(test)]
    TestChannel {
        sender: mpsc::Sender<Bytes>,
        resize_tx: watch::Sender<(u16, u16, u32, u32)>,
        clock: super::submission_clock::SubmissionClock,
    },
}

impl PaneRuntimeIo {
    pub(super) fn submission_clock(&self) -> super::submission_clock::SubmissionClock {
        #[cfg(test)]
        if let Self::TestChannel { clock, .. } = self {
            return clock.clone();
        }
        super::submission_clock::SubmissionClock::default()
    }

    pub(super) fn shutdown(&self) {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.shutdown(),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => {}
        }
    }

    #[cfg(unix)]
    pub(super) fn foreground_process_group_id(&self) -> Option<u32> {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.foreground_process_group_id(),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => None,
        }
    }

    pub(super) fn resize(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        terminal_responses: Vec<Bytes>,
    ) {
        match self {
            PaneRuntimeIo::Actor(actor) => {
                actor.resize(
                    rows,
                    cols,
                    cell_width_px,
                    cell_height_px,
                    terminal_responses,
                );
            }
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { resize_tx, .. } => {
                let _ = resize_tx.send((rows, cols, cell_width_px, cell_height_px));
            }
        }
    }

    pub(super) fn try_send_bytes(
        &self,
        bytes: Bytes,
    ) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.try_write_user_input(bytes),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { sender, .. } => sender.try_send(bytes),
        }
    }

    pub(super) fn write_terminal_response(&self, response: impl FnOnce() -> Option<Bytes>) {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.write_terminal_response(response),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { sender, .. } => {
                if let Some(bytes) = response() {
                    let _ = sender.try_send(bytes);
                }
            }
        }
    }

    pub(super) fn queue_user_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<std::sync::mpsc::Receiver<std::io::Result<()>>> {
        self.queue_input_write(text, enter, delay, deadline)
            .map(InputCompletion::into_receiver)
    }

    pub(super) fn input_scope(&self) -> InputScope {
        match self {
            #[cfg(unix)]
            Self::Actor(actor) => actor.input_scope(),
            #[cfg(windows)]
            Self::Actor(_) => InputScope::default(),
            #[cfg(test)]
            Self::TestChannel { .. } => InputScope::default(),
        }
    }

    pub(super) fn queue_input_write(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<InputCompletion> {
        self.queue_input_write_scoped(text, enter, delay, deadline, &InputScope::default())
    }

    pub(super) fn queue_input_write_scoped(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        deadline: Option<std::time::Instant>,
        scope: &InputScope,
    ) -> std::io::Result<InputCompletion> {
        let allowance = if !text.is_empty() && !enter.is_empty() {
            delay
        } else {
            std::time::Duration::ZERO
        };
        match self {
            PaneRuntimeIo::Actor(actor) => {
                #[cfg(windows)]
                {
                    let _ = scope;
                    actor
                        .queue_user_input_submission(text, enter, delay, deadline)
                        .map(InputCompletion::from)
                }
                #[cfg(unix)]
                actor.queue_user_input_submission_scoped(text, enter, delay, deadline, scope)
            }
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { sender, clock, .. } => {
                let failure = clock.take_write_failure();
                let _ = (deadline, scope);
                let sender = sender.clone();
                let (reply_tx, reply_rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let result = (if text.is_empty() {
                        Ok(())
                    } else {
                        sender.try_send(text).map_err(std::io::Error::other)
                    })
                    .and_then(|()| {
                        std::thread::sleep(delay);
                        if enter.is_empty() {
                            Ok(())
                        } else {
                            sender.try_send(enter).map_err(std::io::Error::other)
                        }
                    });
                    let result = match failure {
                        Some(fault) => fault.complete(result),
                        None => result,
                    };
                    let _ = reply_tx.send(result);
                });
                Ok(InputCompletion::from(reply_rx))
            }
        }
        .map(|completion| completion.with_enter_delay_allowance(allowance))
    }
}

impl TerminalRuntime {
    /// Developer key/text/paste events, and pane or agent API input, use this
    /// path. Terminal replies, focus reports, mouse, and Bus dialog automation
    /// do not count as typing.
    pub(crate) fn try_send_developer_bytes(
        &self,
        bytes: Bytes,
    ) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        self.invalidate_shell_prompt();
        let result = super::draft_input::DraftInput::send(&self.draft_input, &self.io, bytes);
        if result.is_ok() {
            // An Ok that only buffers for a preservation lease still counts:
            // the bytes were accepted. A full channel leaves the placeholder
            // deliverable because the screen did not change.
            self.startup_suggestion_open.store(false, Ordering::Release);
        }
        result
    }
    pub(crate) fn try_send_developer_paste(
        &self,
        text: String,
    ) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        self.try_send_developer_bytes(self.paste_payload(text))
    }
    /// Resize if the dimensions actually changed.
    pub fn resize(&self, rows: u16, cols: u16, cell_width_px: u32, cell_height_px: u32) {
        let rows = rows.max(2);
        let cols = cols.max(4);
        let size = (rows, cols, cell_width_px, cell_height_px);
        if self.current_size.get() == size {
            return;
        }
        self.current_size.set(size);
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.content_seq.fetch_add(1, Ordering::AcqRel);
        let terminal_responses = self
            .terminal
            .resize(rows, cols, cell_width_px, cell_height_px);
        self.content_seq.fetch_add(1, Ordering::Release);
        drop(_content_write_guard);
        self.compression.wake();
        mark_detection_content_changed(&self.detection_content_seq);
        self.io.resize(
            rows,
            cols,
            cell_width_px,
            cell_height_px,
            terminal_responses,
        );
    }

    pub fn keyboard_protocol(&self) -> crate::protocol::keys::KeyboardProtocol {
        let fallback = crate::protocol::keys::KeyboardProtocol::from_kitty_flags(
            self.kitty_keyboard_flags.load(Ordering::Relaxed),
        );
        self.terminal.keyboard_protocol(fallback)
    }

    pub fn modify_other_keys_level(&self) -> u8 {
        self.terminal.modify_other_keys_level()
    }

    pub fn encode_terminal_key(&self, key: crate::protocol::keys::TerminalKey) -> Vec<u8> {
        self.terminal
            .encode_terminal_key(key, self.keyboard_protocol())
    }

    pub fn try_send_bytes(&self, bytes: Bytes) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        if !bytes.is_empty() {
            self.invalidate_shell_prompt();
            self.invalidate_submission_ownership();
        }
        self.io.try_send_bytes(bytes)
    }

    pub fn queue_user_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<InputCompletion> {
        self.invalidate_shell_prompt();
        self.invalidate_submission_ownership();
        self.io.queue_input_write(text, enter, delay, deadline)
    }

    pub(crate) fn invalidate_submission_ownership(&self) {
        super::draft_input::DraftInput::invalidate(&self.draft_input);
    }

    pub(crate) fn observe_submission_agent_state(
        &self,
        state: crate::agents::AgentState,
        at: std::time::Instant,
    ) {
        super::draft_input::DraftInput::observe_turn(&self.draft_input, state, at);
    }

    pub fn try_send_focus_event(&self, event: crate::terminal::vt::FocusEvent) -> bool {
        if !self.focus_reporting_enabled() {
            return false;
        }

        let Ok(bytes) = crate::terminal::vt::encode_focus(event) else {
            return false;
        };
        if let Err(err) = self.io.try_send_bytes(Bytes::from(bytes)) {
            warn!(err = %err, ?event, "failed to forward pane focus event");
        }
        true
    }

    pub fn wheel_routing(&self) -> Option<WheelRouting> {
        self.terminal.wheel_routing()
    }

    pub fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::protocol::keys::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        if !self.mouse_reporting_enabled() {
            return None;
        }
        self.terminal.encode_mouse_button(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::protocol::keys::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.terminal.encode_mouse_motion(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::protocol::keys::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        if self.wheel_routing()? != WheelRouting::MouseReport {
            return None;
        }
        self.terminal.encode_mouse_wheel(kind, position, modifiers)
    }

    pub(crate) fn pixel_size(&self) -> Option<(u32, u32)> {
        let (rows, cols, cell_width_px, cell_height_px) = self.current_size.get();
        let width = u32::from(cols).checked_mul(cell_width_px)?;
        let height = u32::from(rows).checked_mul(cell_height_px)?;
        (width > 0 && height > 0).then_some((width, height))
    }

    pub fn encode_alternate_scroll(
        &self,
        kind: crossterm::event::MouseEventKind,
    ) -> Option<Vec<u8>> {
        if self.wheel_routing()? != WheelRouting::AlternateScroll {
            return None;
        }
        let key = match kind {
            crossterm::event::MouseEventKind::ScrollUp => crossterm::event::KeyCode::Up,
            crossterm::event::MouseEventKind::ScrollDown => crossterm::event::KeyCode::Down,
            _ => return None,
        };
        Some(
            self.encode_terminal_key(crate::protocol::keys::TerminalKey::new(
                key,
                crossterm::event::KeyModifiers::empty(),
            )),
        )
    }
}
