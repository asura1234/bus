//! The screen an agent's provider left when it exited without Bus asking.
use super::{io, schema, BusState, Method, Path, ResponseResult, RoomAgent, Worker};
use crate::messaging::{identity, model::ExitSnapshot, native::Transport};

/// Scrollback lines saved with the visible screen.
const SCROLLBACK_LINES: u32 = 200;
/// Non-blank lines quoted in the exit notices.
const TAIL_LINES: usize = 20;
/// Characters kept of each quoted line, and of the whole quote, so the exit
/// notice fits one orchestrator notification; the file keeps everything.
const TAIL_LINE_CHARS: usize = 160;
const TAIL_CHARS: usize = 1_600;

/// Save the exited provider's screen once per launch and terminal, before anything else
/// touches the pane. A worker's terminal is closed only after this record is
/// durable (`Worker::close_exited_terminals`); a MASTER orchestrator's stays
/// open for the developer. Startup failures report their own screen.
pub(super) fn save(
    transport: &mut dyn Transport,
    data_dir: &Path,
    state: &mut BusState,
    agent: &RoomAgent,
    pane: &str,
    terminal: &str,
) -> Result<(), String> {
    let launch_id = agent.runtime_identity.launch_id.clone();
    if agent.startup.is_some()
        || agent
            .exit_snapshot
            .as_ref()
            .is_some_and(|saved| saved.launch_id == launch_id && saved.terminal_id == terminal)
    {
        return Ok(());
    }
    let at_ms = io::now_ms();
    let visible = read(transport, pane, schema::ReadSource::Visible, None);
    let recent = read(
        transport,
        pane,
        schema::ReadSource::Recent,
        Some(SCROLLBACK_LINES),
    );
    let mut snapshot = ExitSnapshot {
        launch_id,
        terminal_id: terminal.into(),
        path: None,
        error: None,
        tail: String::new(),
        at_ms,
        terminal_closed: agent.orchestrates.map(|_| false),
    };
    match (&visible, &recent) {
        (Err(visible), Err(recent)) => {
            snapshot.error = Some(format!(
                "screen unreadable (visible: {visible}; scrollback: {recent})"
            ));
        }
        _ => {
            let path = data_dir
                .join("exit-snapshots")
                .join(format!("a{}-{at_ms}.txt", agent.id.0));
            let section = |read: &Result<String, String>| match read {
                Ok(text) => text.clone(),
                Err(error) => format!("(unavailable: {error})"),
            };
            let body = format!(
                "Bus exit snapshot\nAgent: {} (agent {}), provider {}\nPane {pane}, terminal {terminal}, launch {}\nCaptured at {at_ms} ms\n\n--- Visible screen ---\n{}\n\n--- Last {SCROLLBACK_LINES} lines of scrollback ---\n{}\n",
                agent.name,
                agent.id.0,
                identity::provider_kind(agent.provider).label(),
                snapshot.launch_id.as_deref().unwrap_or("unknown"),
                section(&visible),
                section(&recent),
            );
            let written = std::fs::create_dir_all(data_dir.join("exit-snapshots"))
                .and_then(|()| std::fs::write(&path, body));
            match written {
                Ok(()) => {
                    let screen = recent.as_ref().or(visible.as_ref()).map_or("", |s| s);
                    snapshot.tail = tail(screen);
                    snapshot.path = Some(path);
                }
                Err(error) => snapshot.error = Some(format!("could not save it: {error}")),
            }
        }
    }
    tracing::info!(event = "bus.agent.exit_snapshot", agent_id = agent.id.0,
        pane_id = pane, terminal_id = terminal, path = ?snapshot.path,
        error = ?snapshot.error, "Saved the exited provider's screen");
    state
        .record_exit_snapshot(agent.id, snapshot)
        .map_err(|e| e.to_string())
}

impl Worker {
    /// Close each worker terminal whose exit snapshot is now durable, even
    /// with a command running in its shell, then report the exit. A refused
    /// close is recorded and not retried.
    pub(super) fn close_exited_terminals(&mut self) -> Result<(), String> {
        let pending: Vec<_> = self
            .state
            .agents()
            .filter(|agent| {
                !agent.deletion_pending
                    && agent.exit_snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot.close_pending()
                            && snapshot.launch_id == agent.runtime_identity.launch_id
                            && Some(snapshot.terminal_id.as_str())
                                == agent.runtime_identity.terminal_id.as_deref()
                    })
            })
            .cloned()
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let mut state = self.state.clone();
        for agent in pending {
            let Some(mut snapshot) = agent.exit_snapshot.clone() else {
                continue;
            };
            let closed = agent.runtime_identity.pane_id.as_deref().map(|pane| {
                self.request_exited_shell_close(
                    pane,
                    &snapshot.terminal_id,
                    identity::provider_kind(agent.provider).label(),
                    &identity::managed_name(agent.room_id, agent.id),
                    agent.runtime_identity.session_id.clone(),
                )
            });
            if let Some(Err(error)) = &closed {
                tracing::warn!(event = "bus.agent.exit_terminal_kept", agent_id = agent.id.0,
                    code = ?error.code, detail = %error.message,
                    "Exited provider's terminal was not closed");
            }
            snapshot.terminal_closed = Some(matches!(closed, Some(Ok(()))));
            if snapshot.terminal_closed == Some(true) {
                let mut identity = agent.runtime_identity.clone();
                identity.pane_id = None;
                state
                    .set_agent_runtime_identity(agent.id, identity)
                    .map_err(|e| e.to_string())?;
            }
            tracing::info!(event = "bus.agent.exit_terminal", agent_id = agent.id.0,
                terminal_id = %snapshot.terminal_id, closed = snapshot.terminal_closed,
                "Handled the exited provider's terminal");
            state
                .record_exit_snapshot(agent.id, snapshot)
                .map_err(|e| e.to_string())?;
            super::poll::report_exit(&mut state, agent.id, Some(true))?;
        }
        self.save(state)
    }
}

fn read(
    transport: &mut dyn Transport,
    pane: &str,
    source: schema::ReadSource,
    lines: Option<u32>,
) -> Result<String, String> {
    match transport.request(Method::PaneRead(schema::PaneReadParams {
        pane_id: pane.into(),
        source,
        lines,
        format: schema::ReadFormat::Text,
        strip_ansi: true,
        intent: Default::default(),
    })) {
        Ok(ResponseResult::PaneRead { read }) if read.pane_id == pane => Ok(read.text),
        Ok(_) => Err("unexpected terminal read response".into()),
        Err(error) => Err(error.message),
    }
}

/// The last non-blank lines, quoted, stripped of control characters and
/// bounded; the newest lines are kept.
fn tail(screen: &str) -> String {
    let mut quoted = Vec::new();
    let mut chars = 0;
    for line in screen.lines().rev().filter(|line| !line.trim().is_empty()) {
        let line: String = line
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .take(TAIL_LINE_CHARS)
            .collect();
        let line = format!("> {}", defang(&line));
        chars += line.chars().count() + 1;
        if quoted.len() == TAIL_LINES || chars > TAIL_CHARS {
            break;
        }
        quoted.push(line);
    }
    quoted.reverse();
    if quoted.is_empty() {
        "> (screen is empty)".into()
    } else {
        quoted.join("\n")
    }
}

/// Screen text is the worker's output, not Bus asking the developer for
/// anything: its waiting markers get no-break spaces, so they read the same
/// but never page anyone (`feishu::waiting`).
fn defang(line: &str) -> String {
    const MARKERS: [&str; 3] = ["waiting on you", "waiting-on-you", "waiting_on_you"];
    let folded = line.to_ascii_lowercase();
    let mut out = line.to_owned();
    for marker in MARKERS {
        for (start, _) in folded.match_indices(marker) {
            // ASCII lowercasing keeps byte offsets, and the separators are ASCII.
            for offset in [start + 7, start + 10] {
                out.replace_range(offset..offset + 1, "\u{1}");
            }
        }
    }
    out.replace('\u{1}', "\u{a0}")
}
