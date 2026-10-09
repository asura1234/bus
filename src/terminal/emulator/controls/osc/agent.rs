use super::collector::OscStreamCollector;

/// Maximum retained string length for agent OSC title and progress payloads.
/// Title text is untrusted model output; cap it to bound memory and log size.
pub(super) const AGENT_OSC_MAX_CHARS: usize = 256;

/// Always-on tracker that retains the latest OSC 0/2 title and OSC 9 progress
/// payload emitted by the child process. Nothing here affects rendering; this
/// is pure passive capture for the detection engine (Stage C / Stage D).
///
/// - `latest_title` — last OSC 0 or OSC 2 payload, sanitized. An empty
///   payload (e.g. `\x1b]0;\x07`) clears the stored value.
/// - `latest_progress` — last OSC 9 payload (the part after `9;`), stored
///   as-is after sanitization. E.g. `"4;3;"` or `"4;0;"`.
#[derive(Debug, Default)]
pub(in crate::terminal::emulator) struct AgentOscStateTracker {
    collector: OscStreamCollector,
    latest_title: Option<String>,
    terminal_title: Option<String>,
    latest_progress: Option<String>,
}

impl AgentOscStateTracker {
    pub(in crate::terminal::emulator) fn observe(&mut self, bytes: &[u8]) -> bool {
        let (collector, latest_title, terminal_title, latest_progress) = (
            &mut self.collector,
            &mut self.latest_title,
            &mut self.terminal_title,
            &mut self.latest_progress,
        );
        let mut terminal_title_changed = false;
        collector.observe(bytes, |body| {
            let Some((command, payload)) = parse_agent_osc_body(body) else {
                return;
            };
            match command {
                b"0" | b"2" => {
                    let title = sanitize_agent_osc_string(payload, AGENT_OSC_MAX_CHARS);
                    let title = (!title.is_empty()).then_some(title);
                    terminal_title_changed |= *terminal_title != title;
                    *terminal_title = title.clone();
                    *latest_title = title;
                }
                b"9" => {
                    *latest_progress =
                        Some(sanitize_agent_osc_string(payload, AGENT_OSC_MAX_CHARS));
                }
                _ => {}
            }
        });
        terminal_title_changed
    }

    pub(in crate::terminal::emulator) fn terminal_title(&self) -> Option<&str> {
        self.terminal_title.as_deref()
    }

    /// Returns the latest retained OSC title, or `""` if none has been seen or
    /// the last title was an empty clear.
    pub(in crate::terminal::emulator) fn latest_title(&self) -> &str {
        self.latest_title.as_deref().unwrap_or("")
    }

    /// Returns the latest retained OSC 9 progress payload, or `""` if none.
    pub(in crate::terminal::emulator) fn latest_progress(&self) -> &str {
        self.latest_progress.as_deref().unwrap_or("")
    }

    /// Drops the retained title and progress so a new foreground agent cannot
    /// inherit OSC evidence emitted by a previous process. The in-flight parse
    /// state is kept: a sequence spanning the agent change finalizes normally
    /// and is attributed to the new agent.
    pub(in crate::terminal::emulator) fn clear_retained(&mut self) {
        self.latest_title = None;
        self.latest_progress = None;
    }
}

/// Splits an OSC body at the first `;`, returning `(command, payload)`.
/// Returns `None` if there is no `;`.
pub(super) fn parse_agent_osc_body(body: &[u8]) -> Option<(&[u8], &[u8])> {
    let sep = body.iter().position(|&b| b == b';')?;
    Some((&body[..sep], &body[sep + 1..]))
}

pub(super) fn sanitize_agent_osc_string(payload: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(payload);
    let mut out = String::new();
    for ch in text.chars().filter(|ch| !ch.is_control()).take(max_chars) {
        out.push(ch);
    }
    out
}
