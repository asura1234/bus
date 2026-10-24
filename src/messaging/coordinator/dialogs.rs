//! Reports a steady dialog wait to the room and quietly retires it when it
//! settles. A readable native screen suppresses stale blocked metadata. The
//! notice is signed "bus", once per episode; a blocked MASTER orchestrator gets
//! a typed Bus developer-attention notice instead.
#[cfg(test)]
use super::{
    schema, AgentRuntimeIdentity, BTreeMap, Method, PathBuf, PromptId, Provider, ResponseResult,
    RoomId, RuntimeStatus, Transport,
};
use super::{AgentId, Author, BusState, Draft, Worker};
use crate::messaging::model::DialogNoticeTo;

/// Fallback notice key for blocked list metadata without a dialog id.
pub(super) const BLOCKED: &str = "blocked";
/// Polls a change must hold before Bus reports it, so a redraw never counts.
const STEADY_POLLS: u8 = 2;
/// Common notice sentence; each notice also names the worker and its inspection command.
pub(super) const BLOCKED_MESSAGE: &str = "Blocked, needs help to continue.";

impl Worker {
    /// Retire a closed dialog's notice so the next form can start an episode even
    /// with identical text. Closing posts nothing: the blocked notice already said
    /// how to inspect it, and whoever answered it knows it is gone.
    pub(super) fn finish_closed_dialog(&mut self, id: AgentId) -> Result<(), String> {
        let mut state = self.state.clone();
        state
            .set_dialog_notice_to(id, None)
            .map_err(|error| error.to_string())?;
        state
            .observe_dialog(id, false, crate::messaging::storage::io::now_ms())
            .map_err(|error| error.to_string())?;
        state
            .set_dialog_notice(id, None)
            .map_err(|error| error.to_string())?;
        state
            .set_automatic_dialog_answer(id, None)
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
            // A coherent close retires an automatic reservation immediately;
            // ordinary help notices keep their existing steady-poll policy.
            if agent.deletion_pending
                || !(steady || wait.is_none() && agent.automatic_dialog_answer.is_some())
            {
                continue;
            }
            let notice = agent.dialog_notice.clone();
            if wait.is_none() && notice.is_none() && agent.automatic_dialog_answer.is_none() {
                continue;
            }
            // Native ids change with question/command text and options, but
            // ignore selection redraws. A changed id needs another answer.
            let mut verified_shape = wait.clone();
            if wait.as_deref() != Some(crate::agents::dialog::CODEX_PENDING_QUESTION_ID) {
                // AgentList metadata can outlive an answer or manual close.
                // Never open a pending panel or send keys while verifying.
                if let Ok((_, _, terminal, pane, session)) = self.dialog_identity(id) {
                    let Ok(current) =
                        self.native_dialog(&pane, &terminal, session.as_deref(), false)
                    else {
                        continue;
                    };
                    if current.dialog.is_none() && !current.pending_question {
                        if steady {
                            self.finish_closed_dialog(id)?;
                        } else {
                            self.clear_automatic_dialog_answer(id)?;
                        }
                        continue;
                    }
                    // Missing list metadata can be an unreadable redraw. An
                    // open dialog or pending question keeps earlier help live.
                    if wait.is_none() {
                        continue;
                    }
                    if let Some(dialog) = current.dialog {
                        verified_shape = Some(dialog.id.clone());
                        if wait.as_deref() != Some(BLOCKED)
                            && wait.as_deref() != Some(dialog.id.as_str())
                        {
                            self.dialog_seen.insert(id, (Some(dialog.id), 1));
                            continue;
                        }
                    }
                }
            }
            if !steady {
                continue;
            }
            if wait.is_none() {
                // Keep the existing fallback for agents without a bound pane.
                self.finish_closed_dialog(id)?;
                continue;
            }
            if self.auto_answer_dialog(id, verified_shape.as_deref())? {
                continue;
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
            let to = post_blocked(&mut state, id, wait.as_deref() != Some(BLOCKED))?;
            state
                .set_dialog_notice(id, wait)
                .map_err(|error| error.to_string())?;
            state
                .set_dialog_notice_to(id, to)
                .map_err(|error| error.to_string())?;
            self.save(state)?;
        }
        let state = &self.state;
        self.dialog_seen.retain(|id, _| state.agent(*id).is_some());
        Ok(())
    }
}

/// Bus's notice about a blocked agent in its work room: to the room's
/// orchestrator, which delivers it like any message and wakes the
/// orchestrator, or else to the developer. A blocked MASTER orchestrator tells
/// the developer directly, since MASTER cannot itself be orchestrated.
fn post_blocked(
    state: &mut BusState,
    id: AgentId,
    open_dialog: bool,
) -> Result<Option<DialogNoticeTo>, String> {
    let agent = state.agent(id).ok_or("Unknown agent")?;
    let text = format!(
        "{}. {BLOCKED_MESSAGE}\nInspect: bus agent dialog {}",
        state.agent_message_label(id),
        state.agent_cli_selector(id)
    );
    if state.is_developer_attention_author(agent.room_id, &Author::Agent(id)) {
        return state
            .post_developer_attention(id, &text, crate::messaging::storage::io::now_ms())
            .map(|_| None)
            .map_err(|error| error.to_string());
    }
    post_dialog_notice(state, id, open_dialog, text)
}

/// Blocked and resolved notices share the existing room/recipient policy.
/// Returns who the notice went to.
pub(super) fn post_dialog_notice(
    state: &mut BusState,
    id: AgentId,
    open_dialog: bool,
    text: String,
) -> Result<Option<DialogNoticeTo>, String> {
    let room = state.agent(id).ok_or("Unknown agent")?.room_id;
    if state.room(room).is_none() {
        return Ok(None);
    }
    let now = crate::messaging::storage::io::now_ms();
    let orchestrator = state
        .notice_orchestrator(room, Some(id))
        .filter(|_| open_dialog);
    match orchestrator {
        Some(orchestrator) => state
            .submit_message_from(
                room,
                Draft {
                    text,
                    files: Vec::new(),
                    recipient_ids: [orchestrator].into(),
                },
                Author::Bus,
                now,
            )
            .map(|requests| requests.first().copied().map(DialogNoticeTo::Orchestrator)),
        None => state
            .post_bus_notice(room, None, text, now)
            .map(|_| Some(DialogNoticeTo::Developer)),
    }
    .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "tests/dialogs_test.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/auto_ui_test.rs"]
mod auto_ui_tests;
