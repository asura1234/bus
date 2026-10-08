//! Lets a room know when one of its agents is blocked on something it cannot
//! pass alone: a permission dialog, a question, a trust prompt or any other
//! blocked screen. The blocked agent itself says so, once per episode.
use super::*;

/// The notice key for a blocked screen without a readable dialog.
pub(super) const BLOCKED: &str = "blocked";
/// Polls a change must hold before Bus reports it, so a redraw never counts.
const STEADY_POLLS: u8 = 2;
/// The one text for every blocker. Bus is not an agent and adds no details:
/// the orchestrator (or the Human) looks at the agent's terminal instead.
pub(super) const BLOCKED_MESSAGE: &str = "Blocked, needs help to continue.";

impl Worker {
    /// A confirmed answer can close a dialog between status polls. Retire its
    /// notice so the next form can start an episode even with identical text.
    pub(super) fn finish_answered_dialog(&mut self, id: AgentId) -> Result<(), String> {
        let mut state = self.state.clone();
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
            if !steady || agent.dialog_notice == wait || agent.deletion_pending {
                continue;
            }
            // An unreadable redraw cannot prove a new dialog. Keep the last
            // reported id so the same dialog returning does not post again.
            if wait.as_deref() == Some(BLOCKED) && agent.dialog_notice.is_some() {
                continue;
            }
            // 保留已发出的 blocked 标记，屏幕来回闪时不再把同一条通知发第二次。
            if wait.is_none() && agent.dialog_notice.as_deref() == Some(BLOCKED) {
                continue;
            }
            // Native ids change with question/command text and options, but
            // ignore selection redraws. A changed id needs another answer.
            let starts = wait.is_some();
            let mut state = self.state.clone();
            if starts {
                post_blocked(&mut state, id)?;
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
/// orchestrator, or else to the Human. A blocked MASTER orchestrator tells
/// the Human directly, since MASTER cannot itself be orchestrated.
fn post_blocked(state: &mut BusState, id: AgentId) -> Result<(), String> {
    let agent = state.agent(id).ok_or("Unknown agent")?;
    let room = agent.room_id;
    if state.room(room).is_none() {
        return Ok(());
    }
    let now = crate::bus::io::now_ms();
    let orchestrator = state
        .orchestrator_of(room)
        .filter(|orchestrator| orchestrator.id != id && !orchestrator.deletion_pending)
        .map(|orchestrator| orchestrator.id);
    match orchestrator {
        Some(orchestrator) => state
            .submit_message_from(
                room,
                Draft {
                    text: BLOCKED_MESSAGE.to_owned(),
                    files: Vec::new(),
                    recipient_ids: [orchestrator].into(),
                },
                Author::Agent(id),
                now,
            )
            .map(|_| ()),
        None => state
            .post_to_human(room, id, BLOCKED_MESSAGE.to_owned(), Vec::new(), now)
            .map(|_| ()),
    }
    .map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "tests/dialogs_test.rs"]
mod tests;
