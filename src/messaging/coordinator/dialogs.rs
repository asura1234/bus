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
            if agent.deletion_pending || !steady {
                continue;
            }
            let notice = agent.dialog_notice.clone();
            if wait.is_none() && notice.is_none() {
                continue;
            }
            // List metadata may outlive an answered screen. Verify through the
            // same read-only driver used by orchestrators, with native identity
            // and status checks. Never open a question panel or send input here.
            if self.terminal_identity(id).is_ok() {
                let Ok(current) = self.native_blocker(id) else {
                    continue;
                };
                let Some(current) = current else {
                    self.finish_closed_dialog(id)?;
                    continue;
                };
                if wait.is_none() {
                    continue;
                }
                if wait.as_deref() != Some(BLOCKED)
                    && current != BLOCKED
                    && wait.as_deref() != Some(current.as_str())
                {
                    self.dialog_seen.insert(id, (Some(current), 1));
                    continue;
                }
            }
            if wait.is_none() {
                // Keep the fallback for agents without a bound pane.
                self.finish_closed_dialog(id)?;
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
            // A restart flap, or a result or task for this agent in the same
            // clock minute, is not a dialog a person must answer. Leave the
            // episode unlatched so a dialog that stays open can post once
            // that minute or the restart file is gone.
            if notice_suppressed(self, id) {
                continue;
            }
            let mut state = self.state.clone();
            let to = post_blocked(&mut state, id)?;
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
fn post_blocked(state: &mut BusState, id: AgentId) -> Result<Option<DialogNoticeTo>, String> {
    let agent = state.agent(id).ok_or("Unknown agent")?;
    let text = format!(
        "{}. {BLOCKED_MESSAGE}\nInspect: bus agent observe {}",
        state.agent_message_label(id),
        state.agent_cli_selector(id)
    );
    if state.is_developer_attention_author(agent.room_id, &Author::Agent(id)) {
        return state
            .post_developer_attention(id, &text, crate::messaging::storage::io::now_ms())
            .map(|_| None)
            .map_err(|error| error.to_string());
    }
    post_dialog_notice(state, id, text)
}

/// Blocked and resolved notices share the existing room/recipient policy.
/// Returns who the notice went to.
fn post_dialog_notice(
    state: &mut BusState,
    id: AgentId,
    text: String,
) -> Result<Option<DialogNoticeTo>, String> {
    let room = state.agent(id).ok_or("Unknown agent")?.room_id;
    if state.room(room).is_none() {
        return Ok(None);
    }
    let now = crate::messaging::storage::io::now_ms();
    let orchestrator = state.notice_orchestrator(room, Some(id));
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

const MINUTE_MS: u64 = 60_000;

fn same_minute(left: u64, right: u64) -> bool {
    left / MINUTE_MS == right / MINUTE_MS
}

/// A restart file, a restart-shaped exit, or same-minute traffic for `id`.
fn notice_suppressed(worker: &Worker, id: AgentId) -> bool {
    let now = crate::messaging::storage::io::now_ms();
    restart_suppresses(worker, id, now) || same_minute_activity(worker, id, now)
}

fn restart_suppresses(worker: &Worker, id: AgentId, now: u64) -> bool {
    let root = &worker.data_dir;
    // A marker left by an interrupted helper has no timestamp of its own.
    // Age is against this `now`, so a day later is not still "in progress".
    if restart_marker_in_window(root, now)
        || crate::utils::restart_record::outcome_path(root).exists()
    {
        return true;
    }
    let Some(snapshot) = worker
        .state
        .agent(id)
        .and_then(|agent| agent.exit_snapshot.as_ref())
    else {
        return false;
    };
    // A snapshot stays until relaunch. Suppress while a restart log falls in
    // this clock minute, including a snapshot saved on this poll.
    restart_logged_same_minute(root, snapshot.at_ms.max(now))
}

/// A handoff waits one 45s health budget, then the same budget again on
/// rollback. Messaging does not call the CLI for that duration. Past 90s,
/// the marker is not a live restart.
fn restart_marker_in_window(root: &std::path::Path, now: u64) -> bool {
    const WINDOW_MS: u64 = 90_000;
    let path = crate::utils::restart_record::marker_path(root);
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return true;
    };
    let Ok(written) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return true;
    };
    let written_ms = u64::try_from(written.as_millis()).unwrap_or(u64::MAX);
    now.saturating_sub(written_ms) <= WINDOW_MS
}

fn restart_logged_same_minute(root: &std::path::Path, stamp: u64) -> bool {
    let Ok(text) = std::fs::read_to_string(crate::utils::restart_record::log_path(root)) else {
        return false;
    };
    text.lines().any(|line| {
        let line = line.trim();
        !line.is_empty()
            && serde_json::from_str::<crate::utils::restart_record::RestartLog>(line)
                .is_ok_and(|record| same_minute(record.at_ms, stamp))
    })
}

fn same_minute_activity(worker: &Worker, id: AgentId, now: u64) -> bool {
    let current = worker
        .state
        .agent(id)
        .and_then(|agent| agent.current_request);
    // The live episode points at its Bus notice. That identity is the notice,
    // not `bus_generated` (a first turn is Bus-generated too) and not the
    // sentence (a person can quote it in a real task).
    let notice_ids: Vec<_> = worker
        .state
        .agents()
        .filter_map(|agent| match agent.dialog_notice_to {
            Some(DialogNoticeTo::Orchestrator(request)) => Some(request),
            _ => None,
        })
        .collect();
    let reconciled = worker.state.requests().any(|request| {
        if request.prompt.author == Author::Bus && notice_ids.contains(&request.id) {
            return false;
        }
        let submitted = same_minute(request.prompt.submitted_at_ms, now);
        // The turn already on screen is the dialog's own work. A task that is
        // still queued, or any other prompt submitted this minute, is new work.
        let task = request.agent_id == id
            && request.prompt.author != Author::Bus
            && submitted
            && Some(request.id) != current;
        let sent = request.prompt.author == Author::Agent(id) && submitted;
        let result = request.agent_id == id
            && request.reply().is_some()
            && request
                .completed_at_ms
                .is_some_and(|at| same_minute(at, now));
        task || sent || result
    });
    reconciled || spooled_turn_final(worker, id, now)
}

/// A Stop can already be in the callback spool when this poll decides. Tick
/// applies that spool only after `notify_dialogs`, so the reconciled state
/// still looks idle. Treat a final for the in-flight turn as a result.
fn spooled_turn_final(worker: &Worker, id: AgentId, now: u64) -> bool {
    let (launch, provider, request_id) = {
        let Some(agent) = worker.state.agent(id) else {
            return false;
        };
        (
            agent.runtime_identity.launch_id.clone(),
            agent.provider,
            agent.current_request,
        )
    };
    let Some(launch) = launch else {
        return false;
    };
    let Some(request_id) = request_id else {
        return false;
    };
    let Some(request) = worker.state.request(request_id) else {
        return false;
    };
    let prompt_id = request.provider_prompt_id.clone();
    let turn_id = request.provider_turn_id.clone();
    let Ok(records) =
        crate::agents::providers::spool::records(&worker.data_dir.join("callbacks").join(&launch))
    else {
        return false;
    };
    // Session start can still be missing, so the turn is not bound yet. A
    // final already sitting on this launch is still the result. Once the turn
    // is bound, only that turn's final counts.
    let unbound = prompt_id.is_none() && turn_id.is_none();
    records.into_iter().any(|(_, record)| {
        if record.nested_provider
            || record.manifest.routing_key != crate::agents::providers::spool::RoutingKey(id.0)
            || record.manifest.launch_id != launch
            || record.manifest.provider != provider
            || !same_minute(record.at_ms, now)
        {
            return false;
        }
        let Ok(parsed) = crate::agents::providers::parse(provider, &record.value) else {
            return false;
        };
        let turn = match parsed {
            crate::agents::providers::spool::Parsed::Final { turn, .. }
            | crate::agents::providers::spool::Parsed::Completed { turn, .. }
            | crate::agents::providers::spool::Parsed::Failure { turn, .. } => turn,
            _ => return false,
        };
        unbound
            || Some(turn.as_str()) == prompt_id.as_deref()
            || Some(turn.as_str()) == turn_id.as_deref()
    })
}

#[cfg(test)]
#[path = "tests/dialogs_test.rs"]
mod tests;
