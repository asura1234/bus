//! An orchestrator's status report. A turn that answers a message ends in that
//! message's reply, which shows in the message's own room: MASTER for the
//! developer's message, the work room for a worker's (a worker said it is
//! done or blocked). When a MASTER orchestrator ends a turn no message started
//! (a background command exited), that turn's final text is routine status,
//! so Bus shows it in the work room the orchestrator runs as its own message to
//! the developer. MASTER carries only what needs the developer: the replies to
//! their MASTER messages, and what the orchestrator posts there itself with
//! `send --to human`. A final that arrives after its request already closed
//! (`reply_not_captured`) still shows in that request's room, so a late answer
//! to the developer's MASTER message stays in MASTER. Bus authors nothing: the
//! text is the agent's.
use super::{
    AgentId, Author, BusState, CallbackDisposition, CallbackEventKind, CallbackRejection,
    ProviderCallback, RoomId,
};
use crate::messaging::model::RequestPhase;

/// An orchestrator's final text, taken before `accept_callback` settles it.
pub(super) struct PendingReport {
    master: RoomId,
    /// Where the report shows: the room of the message whose turn it ends,
    /// else the work room the orchestrator runs.
    room: RoomId,
    agent: AgentId,
    text: String,
    /// The provider session and turn the final ends.
    session: Option<String>,
    turn: Option<String>,
    launch: String,
    key: String,
    completed_owner: bool,
    already_answered: bool,
}

impl PendingReport {
    pub(super) fn answers_completed_turn(&self) -> bool {
        self.completed_owner
    }
}

/// The report a final callback would make, if its agent is in MASTER. Workers
/// never report to MASTER, and a final without any word is not a report.
pub(super) fn pending_report(
    state: &BusState,
    callback: &ProviderCallback,
) -> Option<PendingReport> {
    let CallbackEventKind::Final { text } = &callback.kind else {
        return None;
    };
    let master = state.master_room()?.id;
    let agent = state.agent(callback.agent_id)?;
    if agent.room_id != master || !text.chars().any(char::is_alphanumeric) {
        return None;
    }
    // A final that arrives after its request closed (say as
    // `reply_not_captured`) still answers that request's message, so it
    // belongs in the message's room; only a turn no message started goes to
    // the work room.
    let owner = callback.provider_turn_id.as_deref().and_then(|turn| {
        state.requests().find(|request| {
            request.agent_id == agent.id
                && request.expected_launch_id.as_deref() == Some(&callback.launch_id)
                && request.trusted_start_bound
                && request
                    .submission_boundary
                    .is_some_and(|b| callback.sequence > b)
                && request.provider_turn_id.as_deref() == Some(turn)
                && request.provider_session_id == callback.provider_session_id
                && request.provider_prompt_id.as_ref().is_none_or(|prompt| {
                    callback
                        .provider_prompt_id
                        .as_ref()
                        .is_none_or(|actual| actual == prompt)
                })
        })
    });
    let completed_owner = owner.is_some_and(|request| {
        request.phase == RequestPhase::Completed && request.pending_final.is_some()
    });
    let already_answered = completed_owner
        && owner.is_some_and(|request| {
            request
                .pending_final
                .as_ref()
                .is_some_and(|answer| answer.text.trim() == text.trim())
        });
    let room = owner
        .map(|request| request.room_id)
        .or(agent.orchestrates)
        .unwrap_or(master);
    Some(PendingReport {
        master,
        room,
        agent: agent.id,
        text: text.trim().to_owned(),
        session: callback.provider_session_id.clone(),
        turn: callback.provider_turn_id.clone(),
        launch: callback.launch_id.clone(),
        key: format!(
            "report-final:{}\0{}\0{}\0{}\0{}",
            agent.id.0,
            callback.launch_id,
            callback.provider_session_id.as_deref()?,
            callback.provider_turn_id.as_deref()?,
            crate::utils::time::digest(text.trim().as_bytes()),
        ),
        completed_owner,
        already_answered,
    })
}

/// Posts the report once its callback is applied. Only a final ending a turn
/// Bus did not start is a report; a final accepted for a request is that
/// request's reply and already shows in its room. Changed finals of a completed,
/// attested turn are separate follow-ups, each published once by text identity.
pub(super) fn post_report(
    state: &mut BusState,
    report: PendingReport,
    disposition: &CallbackDisposition,
    now_ms: u64,
) {
    let reports = match disposition {
        CallbackDisposition::AcceptedCompleted | CallbackDisposition::AcceptedPendingSettlement => {
            false
        }
        CallbackDisposition::Rejected(
            CallbackRejection::UnrelatedTurn | CallbackRejection::NoActiveRequest,
        ) => true,
        CallbackDisposition::Rejected(
            CallbackRejection::DuplicateFinal
            | CallbackRejection::WrongTurn
            | CallbackRejection::WrongPrompt
            | CallbackRejection::UnboundFinal
            | CallbackRejection::BeforeSubmissionBoundary,
        ) if report.completed_owner => true,
        _ => return,
    };
    if state.has_reported_final(&report.key) {
        return;
    }
    if report.already_answered {
        state.record_reported_final(report.key, report.launch);
        return;
    }
    // The orchestrator already posted this text to the developer itself with
    // `send --to human` during this turn, so MASTER has it and the work room
    // needs no copy; an earlier turn's same words are a separate report.
    let first = state.report_turn_start(
        report.master,
        report.agent,
        report.turn.as_deref().map(|turn| (&report.session, turn)),
    );
    let already_posted = state
        .notices(report.master)
        .rev()
        .take_while(|notice| notice.id.0 >= first)
        .find(|notice| notice.author == Author::Agent(report.agent))
        .is_some_and(|notice| notice.text.trim() == report.text);
    if reports && !already_posted {
        match state.post_to_developer(report.room, report.agent, report.text, Vec::new(), now_ms) {
            Ok(message) => tracing::info!(
                event = "bus.report.posted",
                agent_id = report.agent.0,
                room_id = report.room.0,
                message_id = message.0,
                "Orchestrator report shown"
            ),
            Err(error) => {
                tracing::warn!(event = "bus.report.failed",
                    agent_id = report.agent.0, room_id = report.room.0, %error,
                    "Orchestrator report not shown");
                return;
            }
        }
    }
    state.record_reported_final(report.key, report.launch);
    if let Some(turn) = report.turn {
        state.end_report_turn(report.master, report.agent, report.session, turn);
    }
}

#[cfg(test)]
#[path = "tests/reports_test.rs"]
mod tests;
