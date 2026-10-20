//! The screen an agent's provider left when it exited without Bus asking.
use super::{io, schema, BusState, Method, Path, ResponseResult, RoomAgent};
use crate::messaging::{identity, model::ExitSnapshot, native::Transport};

/// Scrollback lines saved with the visible screen.
const SCROLLBACK_LINES: u32 = 200;
/// Non-blank lines quoted in the exit notices.
const TAIL_LINES: usize = 20;

/// Save the exited provider's screen once per launch, before anything else
/// touches the pane, then close the terminal Bus created for it, even with a
/// command running in its shell. Startup failures report their own screen.
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
            .is_some_and(|saved| saved.launch_id == launch_id)
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
        terminal_closed: false,
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
    // Without a saved screen the terminal is the only evidence; leave it.
    if snapshot.path.is_some() {
        snapshot.terminal_closed = close(transport, agent, pane, terminal);
    }
    tracing::info!(event = "bus.agent.exit_snapshot", agent_id = agent.id.0,
        pane_id = pane, terminal_id = terminal, path = ?snapshot.path,
        error = ?snapshot.error, terminal_closed = snapshot.terminal_closed,
        "Saved the exited provider's screen");
    state
        .record_exit_snapshot(agent.id, snapshot)
        .map_err(|e| e.to_string())
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

/// The last non-blank lines, quoted and stripped of control characters.
fn tail(screen: &str) -> String {
    let lines: Vec<_> = screen
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..]
        .iter()
        .map(|line| {
            let line: String = line
                .chars()
                .filter(|c| !c.is_control() || *c == '\t')
                .take(300)
                .collect();
            format!("> {line}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    if tail.is_empty() {
        "> (screen is empty)".into()
    } else {
        tail
    }
}

/// The server closes it only while the provider's exit is its own fact and
/// no agent or managed name has taken the terminal since.
fn close(transport: &mut dyn Transport, agent: &RoomAgent, pane: &str, terminal: &str) -> bool {
    let result = transport.request(Method::PaneCloseIfIdentity(
        schema::PaneCloseIfIdentityParams {
            pane_id: pane.into(),
            expected_terminal_id: terminal.into(),
            expected_agent: identity::provider_kind(agent.provider).label().into(),
            expected_managed_name: identity::managed_name(agent.room_id, agent.id),
            expected_session_id: agent.runtime_identity.session_id.clone(),
            close_exited_shell: true,
            allow_busy_shell: true,
        },
    ));
    if let Err(error) = &result {
        tracing::warn!(event = "bus.agent.exit_terminal_kept", agent_id = agent.id.0,
            pane_id = pane, code = ?error.code, detail = %error.message,
            "Exited provider's terminal was not closed");
    }
    matches!(result, Ok(ResponseResult::Ok {}))
}
