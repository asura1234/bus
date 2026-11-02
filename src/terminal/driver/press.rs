//! Native terminal input owns semantic navigation state and its enqueue commit.
use super::{input, selection_identity, selection_step, SelectionIdentity, SelectionStep};
use crate::protocol::api::schema::{AgentPressParams, TerminalObservation, TerminalPressResult};
use crate::terminal::TerminalRuntime;
use bytes::Bytes;
use std::sync::Mutex;

#[derive(Default)]
pub(in crate::terminal) struct DriverState {
    // Focus before an enqueued arrow; retained until that logical option changes.
    pending: Mutex<Option<SelectionIdentity>>,
}

pub(crate) fn validate_press(params: &AgentPressParams) -> Result<(), &'static str> {
    if params.select.is_some()
        && (params.text.is_some() || !params.keys.is_empty() || params.submit)
    {
        return Err("Select cannot be combined with text, keys or submit");
    }
    if params.select.is_none() && params.text.is_none() && params.keys.is_empty() && !params.submit
    {
        return Err("Press needs select, text, keys or submit");
    }
    Ok(())
}

pub(crate) fn press(
    runtime: &TerminalRuntime,
    params: AgentPressParams,
    observation: TerminalObservation,
) -> Result<TerminalPressResult, (&'static str, String)> {
    // A coherent read alone is not a write guard: the PTY can repaint between
    // observing and enqueueing. Hold the reader's content lock through enqueue.
    let _content = runtime.lock_content();
    if runtime.synchronized_output_active()
        || !runtime.content_seq().is_multiple_of(2)
        || runtime.content_seq() != observation.content_revision
    {
        return Err((
            "terminal_observation_changed",
            "Terminal changed; observe it again".into(),
        ));
    }
    let mut pending = runtime
        .driver_state()
        .pending
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let focus = selection_identity(&observation.grid, params.select.as_deref()).ok();
    let (mut keys, selection_pending) = if let Some(label) = &params.select {
        let step =
            selection_step(&observation.grid, label).map_err(|e| ("selection_unavailable", e))?;
        if pending.is_some() && *pending == focus {
            (Vec::new(), true)
        } else {
            match step {
                SelectionStep::Move(key) => (vec![key.to_owned()], true),
                SelectionStep::Confirm => (vec!["enter".into()], false),
            }
        }
    } else {
        (params.keys, false)
    };
    // Validate all keys before writing any text.
    let encoded = input::encode_keys_with(&keys, |key| runtime.encode_terminal_key(key))
        .map_err(|e| ("invalid_key", e))?;
    let text_written = params.text.as_ref().is_some_and(|text| !text.is_empty());
    if params.submit {
        keys.push("enter".into());
    }
    write_input(runtime, params.text.as_deref(), encoded, params.submit)
        .map_err(|e| ("terminal_write_failed", e))?;
    if !keys.is_empty() || text_written {
        // Successful nonempty explicit input supersedes the earlier operation.
        // Failed enqueue leaves the pending arrow intact.
        *pending = if selection_pending { focus } else { None };
    }
    Ok(TerminalPressResult {
        keys,
        text_written,
        selection_pending,
        observation,
    })
}

fn write_input(
    runtime: &crate::terminal::TerminalRuntime,
    text: Option<&str>,
    encoded: Vec<Vec<u8>>,
    submit: bool,
) -> Result<(), String> {
    let mut bytes = text.map_or_else(Vec::new, |text| {
        input::paste(text, runtime.bracketed_paste_enabled())
    });
    bytes.extend(encoded.into_iter().flatten());
    if bytes.is_empty() && !submit {
        return Ok(());
    }
    let enter = || {
        runtime.encode_terminal_key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            )
            .into(),
        )
    };
    if submit && text.is_some_and(|text| !text.is_empty()) {
        runtime
            .queue_user_input_submission(
                Bytes::from(bytes),
                Bytes::from(enter()),
                std::time::Duration::from_millis(150),
                None,
            )
            .map(|completion| {
                // This input API reports queueing. Explicitly detach the
                // completion so dropping it does not cancel delayed Enter.
                let _ = completion.into_receiver();
            })
            .map_err(|error| error.to_string())
    } else {
        if submit {
            bytes.extend(enter());
        }
        runtime
            .try_send_developer_bytes(Bytes::from(bytes))
            .map_err(|error| error.to_string())
    }
}
