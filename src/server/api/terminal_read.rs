//! Terminal snapshot reads and their range observations.
use crate::protocol::api::schema;
pub(in crate::server) struct TerminalReadObservation {
    pub text: String,
    pub truncated: bool,
    pub viewport_rows: Option<u16>,
    pub viewport_columns: Option<u16>,
    pub requested_lines: Option<u32>,
    pub returned_lines: u32,
    pub available_lines: Option<u64>,
    pub exhausted: Option<bool>,
    pub revision: u64,
}

pub(in crate::server) fn read_terminal_snapshot(
    terminal: &crate::terminal::TerminalRuntime,
    source: schema::ReadSource,
    format: schema::ReadFormat,
    lines: Option<u32>,
) -> TerminalReadObservation {
    use schema::{ReadFormat, ReadSource};

    let line_limit = lines.map(|lines| lines as usize);
    let recent_lines = line_limit.unwrap_or(80);
    let mut visible_facts = None;
    let snapshot = match (format, source) {
        (ReadFormat::Text, ReadSource::Visible) => {
            if line_limit.is_none() {
                if let Some((text, rows, columns, revision)) =
                    terminal.visible_text_snapshot_with_dimensions()
                {
                    visible_facts = Some((rows, columns, revision));
                    limit_snapshot_lines(text, None)
                } else {
                    limit_snapshot_lines(terminal.visible_text(), None)
                }
            } else {
                limit_snapshot_lines(terminal.visible_text(), line_limit)
            }
        }
        (ReadFormat::Text, ReadSource::Recent) => terminal.recent_text_snapshot(recent_lines),
        (ReadFormat::Text, ReadSource::RecentUnwrapped) => {
            terminal.recent_unwrapped_text_snapshot(recent_lines)
        }
        (ReadFormat::Text, ReadSource::Detection) => {
            limit_snapshot_lines(terminal.detection_text(), line_limit)
        }
        (ReadFormat::Ansi, ReadSource::Visible) => {
            limit_snapshot_lines(terminal.visible_ansi(), line_limit)
        }
        (ReadFormat::Ansi, ReadSource::Recent) => terminal.recent_ansi_snapshot(recent_lines),
        (ReadFormat::Ansi, ReadSource::RecentUnwrapped) => {
            terminal.recent_unwrapped_ansi_snapshot(recent_lines)
        }
        (ReadFormat::Ansi, ReadSource::Detection) => {
            limit_snapshot_lines(terminal.detection_text(), line_limit)
        }
    };
    let (rows, columns) = visible_facts
        .map(|(rows, columns, _)| (rows, columns))
        .unwrap_or_else(|| terminal.current_size());
    observe_snapshot(
        snapshot,
        source,
        lines,
        (rows, columns),
        || {
            terminal.scroll_metrics().map(|metrics| {
                metrics
                    .max_offset_from_bottom
                    .saturating_add(metrics.viewport_rows) as u64
            })
        },
        || {
            visible_facts
                .map(|(_, _, revision)| revision)
                .unwrap_or_else(|| terminal.content_seq())
        },
    )
}

/// Derive a read's range facts from the one snapshot its text came from, so a
/// frozen read and a live read report facts the same way.
pub(in crate::server) fn observe_snapshot(
    snapshot: crate::terminal::emulator::TerminalReadSnapshot,
    source: schema::ReadSource,
    lines: Option<u32>,
    (rows, columns): (u16, u16),
    scrollable_lines: impl FnOnce() -> Option<u64>,
    revision: impl FnOnce() -> u64,
) -> TerminalReadObservation {
    use schema::ReadSource;

    let rendered_rows = snapshot.text.split_inclusive('\n').count() as u32;
    let recent = matches!(source, ReadSource::Recent | ReadSource::RecentUnwrapped);
    let available_lines = recent.then(|| scrollable_lines().unwrap_or(rendered_rows as u64));
    let returned_lines = if recent {
        available_lines
            .unwrap_or_default()
            .min(lines.unwrap_or(80) as u64) as u32
    } else {
        lines.map_or(rendered_rows, |requested| requested.min(rendered_rows))
    };
    TerminalReadObservation {
        text: snapshot.text,
        truncated: snapshot.truncated,
        viewport_rows: (source == ReadSource::Visible).then_some(rows),
        viewport_columns: (source == ReadSource::Visible).then_some(columns),
        requested_lines: lines,
        returned_lines,
        available_lines,
        exhausted: recent.then_some(!snapshot.truncated),
        revision: revision(),
    }
}

pub(crate) fn limit_snapshot_lines(
    text: String,
    limit: Option<usize>,
) -> crate::terminal::emulator::TerminalReadSnapshot {
    let Some(limit) = limit else {
        return crate::terminal::emulator::TerminalReadSnapshot {
            text,
            truncated: false,
        };
    };
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    crate::terminal::emulator::TerminalReadSnapshot {
        text: lines[lines.len().saturating_sub(limit)..].concat(),
        truncated: lines.len() > limit,
    }
}

use crate::server::main_loop::HeadlessServer;
use std::time::Instant;

pub(in crate::server) struct AltScreenReadSpec {
    pub(in crate::server) terminal_id: crate::utils::ids::TerminalId,
    pub(in crate::server) lines: usize,
    pub(in crate::server) unwrap: bool,
    pub(in crate::server) initial: crate::terminal::ScreenSnapshot,
    pub(in crate::server) content_seq: u64,
}

pub(in crate::server) enum AltScreenReadConflict {
    None,
    Frozen(TerminalReadObservation),
    Defer,
}

impl HeadlessServer {
    pub(in crate::server) fn agent_read_not_idle_error(
        &self,
        request: &schema::Request,
    ) -> Option<schema::ErrorBody> {
        use schema::{Method, ReadFormat, ReadSource};

        let Method::AgentRead(params) = &request.method else {
            return None;
        };
        let requested = params.lines?;
        if params.format != ReadFormat::Text
            || !matches!(
                params.source,
                ReadSource::Recent | ReadSource::RecentUnwrapped
            )
        {
            return None;
        }
        let target = self.app.resolve_agent_target(&params.target).ok()?;
        let terminal = self
            .app
            .state
            .terminals
            .values()
            .find(|terminal| terminal.id.as_str() == target.terminal_id)?;
        if terminal.effective_known_agent().is_none()
            || terminal.state == crate::agents::AgentState::Idle
        {
            return None;
        }
        let runtime = self.app.terminal_runtimes.get(&terminal.id)?;
        let (screen, snapshot) = runtime.screen_text_snapshot()?;
        if screen != crate::terminal::vt::ActiveScreen::Alternate
            || snapshot.rows.len() >= requested as usize
        {
            return None;
        }
        let status = crate::agents::manifest::agent_state_label(terminal.state);
        Some(schema::ErrorBody {
            code: "agent_not_idle".into(),
            message: format!(
                "cannot read {requested} lines while {} is {status}: its alternate-screen history can only be captured by scrolling while idle. Wait and retry, or use --source visible",
                params.target
            ),
        })
    }

    pub(in crate::server) fn alt_screen_read_spec(
        &self,
        request: &schema::Request,
    ) -> Option<AltScreenReadSpec> {
        use schema::{Method, ReadFormat, ReadIntent, ReadSource};

        let (target, source, lines, format) = match &request.method {
            Method::AgentRead(params) => (
                self.app.resolve_agent_target(&params.target).ok()?,
                params.source,
                params.lines,
                params.format,
            ),
            Method::PaneRead(params) if params.intent == ReadIntent::Interactive => (
                self.app.resolve_terminal_target(&params.pane_id).ok()?,
                params.source,
                params.lines,
                params.format,
            ),
            _ => return None,
        };
        if format != ReadFormat::Text
            || !matches!(source, ReadSource::Recent | ReadSource::RecentUnwrapped)
        {
            return None;
        }
        let lines = lines.unwrap_or(80) as usize;
        if lines == 0
            || self
                .pending_alt_screen_reads
                .iter()
                .any(|pending| pending.terminal_id.as_str() == target.terminal_id)
        {
            return None;
        }
        let terminal = self
            .app
            .state
            .terminals
            .values()
            .find(|terminal| terminal.id.as_str() == target.terminal_id)?;
        if terminal.effective_known_agent().is_none()
            || terminal.state != crate::agents::AgentState::Idle
        {
            return None;
        }
        let runtime = self.app.terminal_runtimes.get(&terminal.id)?;
        if runtime.wheel_routing() != Some(crate::terminal::runtime::WheelRouting::MouseReport) {
            return None;
        }
        let (screen, initial, content_seq) = runtime.screen_text_snapshot_with_seq()?;
        if screen != crate::terminal::vt::ActiveScreen::Alternate || initial.rows.len() >= lines {
            return None;
        }
        Some(AltScreenReadSpec {
            terminal_id: terminal.id.clone(),
            lines,
            unwrap: source == ReadSource::RecentUnwrapped,
            initial,
            content_seq,
        })
    }

    pub(in crate::server) fn poll_pending_alt_screen_reads(&mut self, now: Instant) {
        let pending = std::mem::take(&mut self.pending_alt_screen_reads);
        for read in pending {
            let runtime = self.app.terminal_runtimes.get(&read.terminal_id);
            let remains_idle = self
                .app
                .state
                .terminals
                .get(&read.terminal_id)
                .is_some_and(|terminal| terminal.state == crate::agents::AgentState::Idle);
            let outcome = if remains_idle {
                read.poll(runtime, now)
            } else {
                read.abort(runtime, now)
            };
            if let Some(read) = outcome {
                self.pending_alt_screen_reads.push(read);
            }
        }
    }

    pub(in crate::server) fn alt_screen_read_conflict(
        &self,
        request: &schema::Request,
    ) -> AltScreenReadConflict {
        let (target, source, lines, format) = match &request.method {
            schema::Method::AgentRead(params) => (
                self.app.resolve_agent_target(&params.target).ok(),
                params.source,
                params.lines,
                params.format,
            ),
            schema::Method::PaneRead(params) => (
                self.app.resolve_terminal_target(&params.pane_id).ok(),
                params.source,
                params.lines,
                params.format,
            ),
            _ => return AltScreenReadConflict::None,
        };
        let Some(target) = target else {
            return AltScreenReadConflict::None;
        };
        let Some(pending) = self
            .pending_alt_screen_reads
            .iter()
            .find(|pending| pending.terminal_id.as_str() == target.terminal_id)
        else {
            return AltScreenReadConflict::None;
        };
        if format == schema::ReadFormat::Text {
            AltScreenReadConflict::Frozen(pending.frozen_read(source, lines))
        } else {
            AltScreenReadConflict::Defer
        }
    }

    pub(in crate::server) fn process_deferred_alt_screen_reads(&mut self) -> bool {
        let deferred = std::mem::take(&mut self.deferred_alt_screen_reads);
        let mut changed = false;
        for msg in deferred {
            match self.alt_screen_read_conflict(&msg.request) {
                AltScreenReadConflict::None => {
                    changed |= self.handle_api_request_with_shutdown_check(msg);
                }
                AltScreenReadConflict::Frozen(_) | AltScreenReadConflict::Defer => {
                    self.deferred_alt_screen_reads.push(msg);
                }
            }
        }
        changed
    }
}
