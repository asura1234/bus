//! Lets a room know when one of its agents is blocked on something it cannot
//! pass alone: a permission dialog, a question, a trust prompt or any other
//! blocked screen. The blocked agent itself says so, once per episode.
#[cfg(test)]
use super::{
    schema, AgentRuntimeIdentity, BTreeMap, Method, ModelError, PathBuf, PromptId, Provider,
    ResponseResult, RoomId, RuntimeStatus, Transport,
};
use super::{AgentId, Author, BusState, Draft, Worker};

/// The notice key for a blocked screen without a readable dialog.
pub(super) const BLOCKED: &str = "blocked";
/// Polls a change must hold before Bus reports it, so a redraw never counts.
const STEADY_POLLS: u8 = 2;
/// Common notice sentence; each notice also names the worker and its inspection command.
pub(super) const BLOCKED_MESSAGE: &str = "Blocked, needs help to continue.";

impl Worker {
    /// A confirmed answer can close a dialog between status polls. Retire its
    /// notice so the next form can start an episode even with identical text.
    pub(super) fn finish_answered_dialog(&mut self, id: AgentId) -> Result<(), String> {
        let mut state = self.state.clone();
        state
            .observe_dialog(id, false)
            .map_err(|error| error.to_string())?;
        state
            .set_dialog_notice(id, None)
            .map_err(|error| error.to_string())?;
        self.save(state)?;
        self.dialog_seen.remove(&id);
        Ok(())
    }

    /// `waits` holds, for each agent the poll saw, its dialog `id`, `BLOCKED`,
    /// or `None` when it waits on nothing. A new readable dialog starts another
    /// episode even if the agent remains blocked; repeats and redraws of that
    /// dialog add nothing.
    pub(super) fn notify_dialogs(
        &mut self,
        waits: Vec<(AgentId, Option<String>)>,
    ) -> Result<(), String> {
        for (id, wait) in waits {
            let steady = match self.dialog_seen.get_mut(&id) {
                Some((seen, polls)) if *seen == wait => {
                    *polls = polls.saturating_add(1);
                    *polls >= STEADY_POLLS
                }
                _ => {
                    self.dialog_seen.insert(id, (wait.clone(), 1));
                    false
                }
            };
            let Some(agent) = self.state.agent(id) else {
                continue;
            };
            if !steady || agent.deletion_pending {
                continue;
            }
            let notice = agent.dialog_notice.clone();
            if wait.is_none() {
                if notice.is_some() {
                    self.finish_answered_dialog(id)?;
                }
                continue;
            }
            // Native ids change with question/command text and options, but
            // ignore selection redraws. A changed id needs another answer.
            let starts = wait.is_some();
            if starts && wait.as_deref() != Some(crate::agents::dialog::CODEX_PENDING_QUESTION_ID) {
                // AgentList metadata can outlive a handler or manual answer.
                // Never open a pending panel or send keys while verifying.
                if let Ok((_, _, terminal, pane, session)) = self.dialog_identity(id) {
                    let Ok(current) =
                        self.native_dialog(&pane, &terminal, session.as_deref(), false)
                    else {
                        continue;
                    };
                    if current.dialog.is_none() && !current.pending_question {
                        self.finish_answered_dialog(id)?;
                        continue;
                    }
                    if let Some(dialog) = current.dialog {
                        if wait.as_deref() != Some(BLOCKED)
                            && wait.as_deref() != Some(dialog.id.as_str())
                        {
                            self.dialog_seen.insert(id, (Some(dialog.id), 1));
                            continue;
                        }
                    }
                }
            }
            if notice == wait {
                continue;
            }
            // An unreadable redraw cannot prove a new dialog. Keep the last
            // reported id so the same dialog returning does not post again.
            if wait.as_deref() == Some(BLOCKED) && notice.is_some() {
                continue;
            }
            let mut state = self.state.clone();
            if starts {
                post_blocked(&mut state, id, wait.as_deref() != Some(BLOCKED))?;
            }
            state
                .set_dialog_notice(id, wait)
                .map_err(|error| error.to_string())?;
            self.save(state)?;
        }
        let state = &self.state;
        self.dialog_seen.retain(|id, _| state.agent(*id).is_some());
        Ok(())
    }
}

/// The blocked agent's own message in its work room: to the room's
/// orchestrator, which delivers it like any agent message and wakes the
/// orchestrator, or else to the developer. A blocked MASTER orchestrator tells
/// the developer directly, since MASTER cannot itself be orchestrated.
fn post_blocked(state: &mut BusState, id: AgentId, open_dialog: bool) -> Result<(), String> {
    let agent = state.agent(id).ok_or("Unknown agent")?;
    let room = agent.room_id;
    let text = format!(
        "{} (agent {}). {BLOCKED_MESSAGE}\nInspect: bus agent dialog {}",
        agent.name, id.0, id.0
    );
    if state.room(room).is_none() {
        return Ok(());
    }
    let now = crate::messaging::storage::io::now_ms();
    let orchestrator = state
        .orchestrator_of(room)
        .filter(|orchestrator| {
            open_dialog && orchestrator.id != id && !orchestrator.deletion_pending
        })
        .map(|orchestrator| orchestrator.id);
    match orchestrator {
        Some(orchestrator) => state
            .submit_message_from(
                room,
                Draft {
                    text,
                    files: Vec::new(),
                    recipient_ids: [orchestrator].into(),
                },
                Author::Agent(id),
                now,
            )
            .map(|_| ()),
        None => state
            .post_to_developer(room, id, text, Vec::new(), now)
            .map(|_| ()),
    }
    .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "tests/dialogs_test.rs"]
mod tests;
