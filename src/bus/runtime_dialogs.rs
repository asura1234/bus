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
    /// `waits` holds, for each agent the poll saw, its dialog `id`, `BLOCKED`,
    /// or `None` when it waits on nothing. A blocked episode starts when an
    /// agent goes from waiting on nothing to waiting on anything; only that
    /// start is reported. A later dialog in the same episode and the episode's
    /// end add nothing.
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
            // 保留已发出的 blocked 标记，屏幕来回闪时不再把同一条通知发第二次。
            if wait.is_none() && agent.dialog_notice.as_deref() == Some(BLOCKED) {
                continue;
            }
            let starts = wait.is_some() && agent.dialog_notice.is_none();
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
/// orchestrator, or else to the Human. MASTER never gets one.
fn post_blocked(state: &mut BusState, id: AgentId) -> Result<(), String> {
    let agent = state.agent(id).ok_or("Unknown agent")?;
    let room = agent.room_id;
    if state
        .room(room)
        .is_none_or(|room| room.kind == RoomKind::Master)
    {
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
#[path = "runtime_dialogs_tests.rs"]
mod tests;
