use super::collector::OscStreamCollector;

/// Reconstructs selected OSC sequences for local evidence capture while
/// debugging agent title/status behavior. This is intentionally passive:
/// nothing here affects terminal rendering or detection state.
#[derive(Debug)]
pub(in crate::terminal::emulator) struct OscDebugTracker {
    pub(super) enabled: bool,
    pub(super) collector: OscStreamCollector,
    pub(super) pending: Vec<OscDebugEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::terminal::emulator) struct OscDebugEvent {
    pub(in crate::terminal::emulator) command: String,
    pub(in crate::terminal::emulator) payload: String,
}

impl OscDebugTracker {
    pub(in crate::terminal::emulator) fn from_env() -> Self {
        Self {
            enabled: osc_debug_enabled_from_env(),
            collector: OscStreamCollector::default(),
            pending: Vec::new(),
        }
    }

    pub(in crate::terminal::emulator) fn observe(&mut self, bytes: &[u8]) {
        if !self.enabled {
            return;
        }
        let (collector, pending) = (&mut self.collector, &mut self.pending);
        collector.observe(bytes, |body| {
            if let Some(event) = parse_osc_debug_event(body) {
                pending.push(event);
            }
        });
    }

    pub(in crate::terminal::emulator) fn drain_pending(&mut self) -> Vec<OscDebugEvent> {
        std::mem::take(&mut self.pending)
    }
}

impl Default for OscDebugTracker {
    fn default() -> Self {
        Self::from_env()
    }
}

pub(super) fn osc_debug_enabled_from_env() -> bool {
    std::env::var("BUS_DEBUG_OSC_EVIDENCE")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub(super) fn parse_osc_debug_event(body: &[u8]) -> Option<OscDebugEvent> {
    let separator = body.iter().position(|byte| *byte == b';')?;
    let command = &body[..separator];
    let payload = &body[separator + 1..];
    if !matches!(command, b"0" | b"2" | b"9" | b"21337") {
        return None;
    }
    Some(OscDebugEvent {
        command: std::str::from_utf8(command).ok()?.to_string(),
        payload: sanitized_osc_debug_payload(payload),
    })
}

pub(super) fn sanitized_osc_debug_payload(payload: &[u8]) -> String {
    const MAX_CHARS: usize = 512;
    let text = String::from_utf8_lossy(payload);
    let mut sanitized = String::new();
    for ch in text.chars().filter(|ch| !ch.is_control()).take(MAX_CHARS) {
        sanitized.push(ch);
    }
    if text.chars().count() > MAX_CHARS {
        sanitized.push_str("...");
    }
    sanitized
}
