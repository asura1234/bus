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
    pub(in crate::terminal::emulator) shell_prompt_generation: u64,
    pub(in crate::terminal::emulator) launch_prompt_generation: Option<(String, u64)>,
    pub(in crate::terminal::emulator) launch_prompt_witness: Option<(String, u64, u64)>,
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
        let shell_prompt_generation = &mut self.shell_prompt_generation;
        let launch_prompt_generation = &mut self.launch_prompt_generation;
        let launch_prompt_witness = &mut self.launch_prompt_witness;
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
                b"133" => {
                    if payload == b"A" {
                        *shell_prompt_generation = shell_prompt_generation.saturating_add(1);
                        return;
                    }
                    let Some((token, sequence)) = private_prompt(payload) else {
                        return;
                    };
                    let token = String::from_utf8_lossy(token).into_owned();
                    if let Some((cycle, _)) = sequence {
                        if launch_prompt_witness
                            .as_ref()
                            .is_some_and(|(seen, previous, _)| seen == &token && cycle <= *previous)
                        {
                            return;
                        }
                    }
                    *shell_prompt_generation = shell_prompt_generation.saturating_add(1);
                    *launch_prompt_generation = Some((
                        token.clone(),
                        sequence.map_or(*shell_prompt_generation, |(cycle, _)| cycle),
                    ));
                    *launch_prompt_witness =
                        sequence.map(|(cycle, attempt)| (token, cycle, attempt));
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

/// A versioned marker is rendered after user PS1 expansion, with a sequence
/// allocated once by the primary-prompt hook. Readline redraws replay it.
fn private_prompt(payload: &[u8]) -> Option<(&[u8], Option<(u64, u64)>)> {
    let mut fields = payload.strip_prefix(b"A;bus=")?.split(|b| *b == b';');
    let token = fields.next()?;
    if token.is_empty() || token.len() > 64 || !token.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let Some(cycle) = fields.next() else {
        return Some((token, None));
    };
    let number = |value: &[u8]| {
        (!value.is_empty() && value.iter().all(u8::is_ascii_digit))
            .then(|| std::str::from_utf8(value).ok()?.parse::<u64>().ok())
            .flatten()
    };
    let cycle = number(cycle.strip_prefix(b"cycle=")?)?;
    let attempt = number(fields.next()?.strip_prefix(b"attempt=")?)?;
    (cycle > 0 && fields.next().is_none()).then_some((token, Some((cycle, attempt))))
}
