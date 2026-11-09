use super::event::{extract_one_event, RawInputEvent};
#[cfg(unix)]
use super::mouse::starts_with_incomplete_default_mouse_sequence;
use super::mouse::{
    discard_complete_orphaned_sgr_mouse_tail, discard_or_buffer_orphaned_sgr_mouse_tail,
    discard_orphaned_sgr_mouse_tail, starts_with_incomplete_orphaned_sgr_mouse_tail,
    starts_with_incomplete_sgr_mouse_sequence,
};
use super::replies::{
    discard_host_reply_csi_tail, split_paste_color_replies,
    starts_with_incomplete_default_color_response, starts_with_incomplete_host_cell_size_report,
    starts_with_incomplete_host_color_scheme_report, starts_with_incomplete_palette_color_response,
};
use super::sequence::{
    control_string, control_string_terminator_for_family, find_subsequence, plausible_osc_tail,
    starts_with_incomplete_utf8_char, ControlString, ControlStringFamily,
};
use super::{BRACKETED_PASTE_END, BRACKETED_PASTE_START, ESC};
use crate::protocol::keys::parse_terminal_key_sequence;
use crate::protocol::keys::TerminalKey;
use crossterm::event::KeyModifiers;

/// Parse raw terminal input bytes into a list of `RawInputEvent`s.
///
/// This directly extracts events without going through a channel, making it
/// suitable for synchronous use.
#[cfg(any(unix, test))]
pub fn parse_raw_input_bytes_sync(data: &[u8]) -> Vec<RawInputEvent> {
    let mut framer = RawInputFramer::default();
    let mut events = framer.push(data);
    events.extend(framer.flush_timeout());
    events
}
#[derive(Default)]
pub(crate) struct RawInputFramer {
    byte_framer: RawInputByteFramer,
}

impl RawInputFramer {
    #[cfg(any(windows, test))]
    pub(crate) fn for_host_input() -> Self {
        Self {
            byte_framer: RawInputByteFramer::for_host_input(),
        }
    }

    pub(crate) fn push(&mut self, data: &[u8]) -> Vec<RawInputEvent> {
        Self::events_from_chunks(self.byte_framer.push(data))
    }

    #[cfg(any(windows, all(test, not(target_os = "macos"))))]
    pub(crate) fn has_pending_input(&self) -> bool {
        self.byte_framer.has_pending_input()
    }

    #[cfg(any(windows, test))]
    pub(crate) fn has_pending_bracketed_paste(&self) -> bool {
        self.byte_framer.has_pending_bracketed_paste()
    }

    pub(crate) fn flush_timeout(&mut self) -> Vec<RawInputEvent> {
        Self::events_from_chunks(self.byte_framer.flush_timeout())
    }

    fn events_from_chunks(chunks: Vec<Vec<u8>>) -> Vec<RawInputEvent> {
        chunks
            .into_iter()
            .filter_map(|chunk| {
                if chunk.as_slice() == [ESC] {
                    return Some(RawInputEvent::Key(
                        TerminalKey::new(crossterm::event::KeyCode::Esc, KeyModifiers::empty())
                            .with_vt_bytes(chunk),
                    ));
                }
                match extract_one_event(&chunk) {
                    Some((event, _)) => {
                        if matches!(event, RawInputEvent::Unsupported) {
                            log_unparsed_input(&chunk, "unsupported");
                        }
                        tracing::debug!(raw_bytes = ?chunk, event = ?event, "raw input event parsed");
                        Some(event)
                    }
                    None => {
                        log_unparsed_input(&chunk, "rejected");
                        None
                    }
                }
            })
            .collect()
    }
}

#[derive(Default)]
pub(crate) struct RawInputByteFramer {
    buffer: Vec<u8>,
    discard_until: Option<ControlStringFamily>,
    discarded_tail_bytes: usize,
    lone_escape_recently_flushed: bool,
    host_color_replies_awaited: u16,
    host_cell_size_replies_awaited: u16,
    host_appearance_reply_awaited: bool,
    held_pending_host_reply_esc: bool,
    host_color_scheme_change_tracking: bool,
    host_appearance_query_on_focus: bool,
    split_coalesced_escape: bool,
    /// Set only so a later read can log that a held fragment completed.
    held_incomplete: bool,
}

pub(super) const HOST_COLOR_QUERY_REPLIES: u16 = 258;
#[cfg(any(unix, test))]
pub(super) const HOST_CELL_SIZE_QUERY_REPLIES: u16 = 1;
pub(super) const MAX_ORPHANED_SGR_MOUSE_TAIL_BYTES: usize = 32;

impl RawInputByteFramer {
    pub(crate) fn for_host_input() -> Self {
        Self::with_host_input_policy(
            crate::platform::capabilities().preserve_legacy_doubled_escape_input,
        )
    }

    pub(super) fn with_host_input_policy(preserve_legacy_doubled_escape_input: bool) -> Self {
        Self {
            split_coalesced_escape: !preserve_legacy_doubled_escape_input,
            ..Self::default()
        }
    }

    pub(crate) fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(data);
        self.drain_available_chunks()
    }

    /// Hold a lone trailing ESC for one idle flush so an OSC 10/11 reply split
    /// at its ESC introducer stitches back together instead of leaking (#549).
    pub(crate) fn host_color_query_sent(&mut self) {
        self.host_color_replies_awaited = HOST_COLOR_QUERY_REPLIES;
        self.held_pending_host_reply_esc = false;
    }

    fn host_appearance_query_sent(&mut self) {
        self.host_appearance_reply_awaited = true;
        self.held_pending_host_reply_esc = false;
    }

    /// Same hold window as `host_color_query_sent`, for the XTWINOPS cell size
    /// reply. Only the Unix client sends this query.
    #[cfg(any(unix, test))]
    pub(crate) fn host_cell_size_query_sent(&mut self) {
        self.host_cell_size_replies_awaited = HOST_CELL_SIZE_QUERY_REPLIES;
        self.held_pending_host_reply_esc = false;
    }

    fn awaiting_host_reply(&self) -> bool {
        self.host_color_replies_awaited > 0
            || self.host_cell_size_replies_awaited > 0
            || self.host_appearance_reply_awaited
    }

    #[cfg(any(unix, test))]
    pub(crate) fn enable_host_color_scheme_change_tracking(&mut self) {
        self.host_color_scheme_change_tracking = true;
    }

    /// Arm the bounded host-reply window when focus gain will emit an appearance query.
    /// If the write or reply fails, a lone Escape is delayed for only one extra flush.
    #[cfg(any(not(windows), test))]
    pub(crate) fn enable_host_appearance_query_on_focus(&mut self) {
        self.host_appearance_query_on_focus = true;
    }

    pub(crate) fn has_pending_input(&self) -> bool {
        !self.buffer.is_empty()
    }

    #[cfg(unix)]
    pub(crate) fn has_pending_lone_escape(&self) -> bool {
        self.buffer.as_slice() == [ESC]
    }

    #[cfg(unix)]
    pub(crate) fn has_pending_incomplete_mouse_sequence(&self) -> bool {
        starts_with_incomplete_sgr_mouse_sequence(&self.buffer)
            || starts_with_incomplete_default_mouse_sequence(&self.buffer)
    }

    pub(crate) fn has_pending_bracketed_paste(&self) -> bool {
        self.buffer.starts_with(BRACKETED_PASTE_START)
            && find_subsequence(&self.buffer, BRACKETED_PASTE_END).is_none()
    }

    pub(crate) fn flush_timeout(&mut self) -> Vec<Vec<u8>> {
        let mut chunks = self.drain_available_chunks();

        if let Some(family) = self.discard_until {
            self.discard_timeout_tail(family);
            return chunks;
        }

        if self.buffer.is_empty() {
            return chunks;
        }

        if self.lone_escape_recently_flushed && self.buffer.starts_with(b"[<") {
            tracing::debug!(
                len = self.buffer.len(),
                "discarding incomplete orphaned SGR mouse tail after input timeout"
            );
            discard_or_buffer_orphaned_sgr_mouse_tail(
                &mut self.buffer,
                &mut self.discard_until,
                &mut self.discarded_tail_bytes,
            );
            self.held_incomplete = false;
            self.lone_escape_recently_flushed = false;
            return chunks;
        }

        if starts_with_incomplete_sgr_mouse_sequence(&self.buffer) {
            self.discard_incomplete_sgr_mouse();
            return chunks;
        }

        if self.hold_or_discard_timeout_reply() {
            return chunks;
        }

        if self.buffer.as_slice() == [ESC] {
            if self.awaiting_host_reply() && !self.held_pending_host_reply_esc {
                self.held_pending_host_reply_esc = true;
                tracing::trace!("holding lone escape one flush while awaiting host reply");
                return chunks;
            }
            // No continuation arrived; give up the window so Escape is not delayed again.
            self.host_color_replies_awaited = 0;
            self.host_cell_size_replies_awaited = 0;
            self.host_appearance_reply_awaited = false;
            self.held_pending_host_reply_esc = false;
            tracing::warn!(
                bytes = ?self.buffer,
                "flushing lone escape after input timeout; if this follows an alt chord or focus switch it may reach the pane as plain esc"
            );
            self.note_sequence_completed(self.buffer.len());
            self.lone_escape_recently_flushed = true;
            chunks.push(std::mem::take(&mut self.buffer));
            return chunks;
        }

        if let Ok(text) = std::str::from_utf8(&self.buffer) {
            if parse_terminal_key_sequence(text).is_some() {
                self.note_sequence_completed(self.buffer.len());
                chunks.push(std::mem::take(&mut self.buffer));
                return chunks;
            }
        }

        if starts_with_incomplete_utf8_char(&self.buffer) {
            tracing::trace!(bytes = ?self.buffer, "waiting for UTF-8 continuation bytes");
            return chunks;
        }

        if self.buffer.first() == Some(&ESC) && starts_with_incomplete_utf8_char(&self.buffer[1..])
        {
            tracing::trace!(bytes = ?self.buffer, "waiting for escaped UTF-8 continuation bytes");
            return chunks;
        }

        tracing::debug!(bytes = ?self.buffer, "dropping incomplete raw input buffer after timeout");
        if !self.buffer.is_empty() {
            log_unparsed_input(&self.buffer, "incomplete");
            tracing::debug!(
                event = "bus.client.input_sequence_discarded",
                len = self.buffer.len(),
            );
        }
        self.held_incomplete = false;
        self.lone_escape_recently_flushed = false;
        self.buffer.clear();
        chunks
    }

    fn discard_incomplete_sgr_mouse(&mut self) {
        let discarded = std::mem::take(&mut self.buffer);
        tracing::debug!(
            bytes = ?discarded,
            "discarding incomplete SGR mouse sequence after input timeout"
        );
        log_unparsed_input(&discarded, "incomplete_sgr");
        tracing::debug!(
            event = "bus.client.input_sequence_discarded",
            len = discarded.len(),
        );
        self.held_incomplete = false;
        self.discarded_tail_bytes = discarded.len();
        self.discard_until = (self.discarded_tail_bytes <= MAX_DISCARDED_CONTROL_TAIL_BYTES)
            .then_some(ControlStringFamily::OrphanedSgrMouseTail);
    }

    fn note_sequence_held(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        tracing::debug!(
            event = "bus.client.input_sequence_held",
            len = self.buffer.len(),
        );
        self.held_incomplete = true;
    }

    fn note_sequence_completed(&mut self, len: usize) {
        if !self.held_incomplete {
            return;
        }
        tracing::debug!(event = "bus.client.input_sequence_completed", len);
        self.held_incomplete = false;
    }

    fn discard_timeout_tail(&mut self, family: ControlStringFamily) {
        let keep_split_st = self.buffer.last() == Some(&ESC);
        let keep_discarding = match family {
            ControlStringFamily::HostReplyCsi => return,
            ControlStringFamily::OrphanedSgrMouseTail => {
                // An empty idle flush means the tail has not arrived yet. The
                // Unix reader flushes twice; clearing here typed `;122;5M`.
                if self.buffer.is_empty() {
                    return;
                }
                self.forget_held_sequence("orphaned_sgr_tail");
                self.discard_until = None;
                self.discarded_tail_bytes = 0;
                return;
            }
            ControlStringFamily::Osc => plausible_osc_tail(&self.buffer),
            ControlStringFamily::StTerminated => keep_split_st,
        };

        self.discarded_tail_bytes = self.discarded_tail_bytes.saturating_add(self.buffer.len());
        self.held_incomplete = false;
        self.buffer.clear();
        if keep_discarding && self.discarded_tail_bytes <= MAX_DISCARDED_CONTROL_TAIL_BYTES {
            if keep_split_st {
                self.buffer.push(ESC);
            }
        } else {
            self.discard_until = None;
            self.discarded_tail_bytes = 0;
        }
    }

    fn hold_or_discard_timeout_reply(&mut self) -> bool {
        if self.has_pending_bracketed_paste() {
            tracing::trace!(
                len = self.buffer.len(),
                "waiting for bracketed paste terminator"
            );
            return true;
        }

        if starts_with_incomplete_default_color_response(&self.buffer) {
            tracing::trace!(
                len = self.buffer.len(),
                "waiting for host color response terminator"
            );
            return true;
        }

        // Palette replies stream in by the hundred after a color query, so one
        // can straddle an idle flush; hold it only while those replies are awaited.
        if self.host_color_replies_awaited > 0
            && starts_with_incomplete_palette_color_response(&self.buffer)
        {
            tracing::trace!(
                len = self.buffer.len(),
                "waiting for host palette color response terminator"
            );
            return true;
        }

        if (self.host_cell_size_replies_awaited > 0 || self.host_appearance_reply_awaited)
            && self.buffer.as_slice() == b"\x1b["
        {
            if !self.held_pending_host_reply_esc {
                self.held_pending_host_reply_esc = true;
                tracing::trace!("holding incomplete host CSI reply one flush");
                return true;
            }
            self.host_cell_size_replies_awaited = 0;
            self.host_appearance_reply_awaited = false;
            self.held_pending_host_reply_esc = false;
        }

        if self.host_cell_size_replies_awaited > 0
            && starts_with_incomplete_host_cell_size_report(&self.buffer)
        {
            tracing::debug!(
                len = self.buffer.len(),
                "discarding incomplete host cell size report after input timeout"
            );
            self.host_cell_size_replies_awaited = 0;
            self.held_pending_host_reply_esc = false;
            self.discard_until = Some(ControlStringFamily::HostReplyCsi);
            self.discarded_tail_bytes = 0;
            self.forget_held_sequence("host_cell_size");
            return true;
        }

        if starts_with_incomplete_host_color_scheme_report(&self.buffer) {
            if self.host_appearance_reply_awaited && !self.held_pending_host_reply_esc {
                self.held_pending_host_reply_esc = true;
                tracing::trace!(
                    len = self.buffer.len(),
                    "holding incomplete host color scheme report one flush"
                );
                return true;
            }
            tracing::debug!(
                len = self.buffer.len(),
                "discarding incomplete host color scheme report after input timeout"
            );
            self.host_appearance_reply_awaited = false;
            self.held_pending_host_reply_esc = false;
            self.discard_until = Some(ControlStringFamily::HostReplyCsi);
            self.discarded_tail_bytes = 0;
            self.forget_held_sequence("host_color_scheme");
            return true;
        }

        if let Some(ControlString::Incomplete { family }) = control_string(&self.buffer) {
            tracing::debug!(
                len = self.buffer.len(),
                "discarding incomplete host control string after input timeout"
            );
            // This intentionally gives host control replies precedence over legacy
            // Alt forms like Alt+] after timeout, so later reply tails cannot leak.
            self.discard_until = Some(family);
            self.discarded_tail_bytes = 0;
            self.forget_held_sequence("host_control");
            return true;
        }
        false
    }

    fn forget_held_sequence(&mut self, state: &'static str) {
        if !self.buffer.is_empty() {
            log_unparsed_input(&self.buffer, state);
            tracing::debug!(
                event = "bus.client.input_sequence_discarded",
                len = self.buffer.len(),
            );
        }
        self.held_incomplete = false;
        self.buffer.clear();
    }

    fn drain_available_chunks(&mut self) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();

        loop {
            if self.lone_escape_recently_flushed {
                if starts_with_incomplete_orphaned_sgr_mouse_tail(&self.buffer) {
                    self.note_sequence_held();
                    break;
                }
                if discard_complete_orphaned_sgr_mouse_tail(&mut self.buffer) {
                    self.lone_escape_recently_flushed = false;
                    continue;
                }
                self.lone_escape_recently_flushed = false;
            }

            if let Some(family) = self.discard_until {
                if family == ControlStringFamily::HostReplyCsi {
                    if discard_host_reply_csi_tail(&mut self.buffer, &mut self.discarded_tail_bytes)
                    {
                        self.discard_until = None;
                        self.discarded_tail_bytes = 0;
                        continue;
                    }
                    break;
                }
                if family == ControlStringFamily::OrphanedSgrMouseTail {
                    if discard_orphaned_sgr_mouse_tail(
                        &mut self.buffer,
                        &mut self.discarded_tail_bytes,
                    ) {
                        self.discard_until = None;
                        self.discarded_tail_bytes = 0;
                        continue;
                    }
                    break;
                }

                let Some(terminator_len) =
                    control_string_terminator_for_family(&self.buffer, family)
                else {
                    break;
                };
                self.buffer.drain(..terminator_len);
                self.discard_until = None;
                self.discarded_tail_bytes = 0;
                continue;
            }

            if self.split_coalesced_escape && self.buffer.starts_with(b"\x1b\x1b") {
                chunks.push(vec![ESC]);
                self.buffer.drain(..1);
                continue;
            }

            if let Some((paste_chunks, consumed)) = split_paste_color_replies(&self.buffer) {
                let replies = u16::try_from(paste_chunks.len() - 1).unwrap_or(u16::MAX);
                self.host_color_replies_awaited =
                    self.host_color_replies_awaited.saturating_sub(replies);
                self.held_pending_host_reply_esc = false;
                chunks.extend(paste_chunks);
                self.buffer.drain(..consumed);
                continue;
            }

            let Some((event, consumed)) = extract_one_event(&self.buffer) else {
                self.note_sequence_held();
                break;
            };
            self.note_sequence_completed(consumed);
            if matches!(
                event,
                RawInputEvent::HostDefaultColor { .. } | RawInputEvent::HostPaletteColors { .. }
            ) {
                self.host_color_replies_awaited = self.host_color_replies_awaited.saturating_sub(1);
            } else if matches!(event, RawInputEvent::HostCellSizeReport { .. }) {
                self.host_cell_size_replies_awaited =
                    self.host_cell_size_replies_awaited.saturating_sub(1);
            } else if self.host_appearance_query_on_focus
                && matches!(event, RawInputEvent::OuterFocusGained)
            {
                self.host_appearance_query_sent();
            } else if matches!(event, RawInputEvent::HostColorSchemeChanged(_)) {
                self.host_appearance_reply_awaited = false;
                if self.host_color_scheme_change_tracking {
                    self.host_color_query_sent();
                }
            }
            self.held_pending_host_reply_esc = false;
            chunks.push(self.buffer[..consumed].to_vec());
            self.buffer.drain(..consumed);
        }

        chunks
    }
}

pub(super) const MAX_DISCARDED_CONTROL_TAIL_BYTES: usize = 128;

const UNPARSED_INPUT_HEX_LIMIT: usize = 64;

/// Bytes the host input parser did not accept as a key, paste, or complete escape.
pub(super) fn log_unparsed_input(bytes: &[u8], state: &'static str) {
    if bytes.is_empty() {
        return;
    }
    tracing::debug!(
        event = "bus.client.input_unparsed",
        bytes = %truncated_input_hex(bytes),
        read_len = bytes.len(),
        state,
    );
}

fn truncated_input_hex(bytes: &[u8]) -> String {
    let shown = bytes.len().min(UNPARSED_INPUT_HEX_LIMIT);
    let mut hex = String::with_capacity(shown * 2 + usize::from(bytes.len() > shown));
    for byte in &bytes[..shown] {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let byte = *byte;
        hex.push(char::from(HEX[usize::from(byte >> 4)]));
        hex.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    if bytes.len() > shown {
        hex.push('…');
    }
    hex
}
