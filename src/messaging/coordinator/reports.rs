//! An orchestrator's report to the Human. When a MASTER orchestrator ends a
//! turn that no message in MASTER started (a background command exited, a
//! worker messaged it, a worker said it is blocked), that turn's final text is
//! its report, so Bus shows it in MASTER as the orchestrator's own message to
//! the Human. Bus authors nothing: the text is the agent's.
use super::*;

/// An orchestrator's final text, taken before `accept_callback` settles it.
pub(super) struct PendingReport {
    master: RoomId,
    agent: AgentId,
    text: String,
    /// The final answers a message posted in MASTER, so it already shows
    /// there as that message's reply.
    shown_in_master: bool,
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
    let shown_in_master = agent.current_request.is_some_and(|lead| {
        std::iter::once(lead)
            .chain(state.group_members(lead))
            .filter_map(|request| state.request(request))
            .any(|request| request.room_id == master)
    });
    Some(PendingReport {
        master,
        agent: agent.id,
        text: text.trim().to_owned(),
        shown_in_master,
    })
}

/// Posts the report once its callback is applied. A final accepted for a
/// request outside MASTER (a worker's message; the request itself settles once
/// the agent is idle) or ending a turn Bus did not start is a report; a
/// duplicate or rejected final is not. Each final callback is
/// applied once, so a turn reports at most once.
pub(super) fn post_report(
    state: &mut BusState,
    report: PendingReport,
    disposition: &CallbackDisposition,
    now_ms: u64,
) {
    let reports = match disposition {
        CallbackDisposition::AcceptedCompleted | CallbackDisposition::AcceptedPendingSettlement => {
            !report.shown_in_master
        }
        CallbackDisposition::Rejected(
            CallbackRejection::UnrelatedTurn | CallbackRejection::NoActiveRequest,
        ) => true,
        _ => false,
    };
    if !reports {
        return;
    }
    // The orchestrator already posted this text itself with `send --to human`.
    let already_posted = state.room(report.master).is_some_and(|room| {
        room.notices
            .iter()
            .rev()
            .find(|notice| notice.author == Author::Agent(report.agent))
            .is_some_and(|notice| notice.text.trim() == report.text)
    });
    if already_posted {
        return;
    }
    if let Err(error) =
        state.post_to_human(report.master, report.agent, report.text, Vec::new(), now_ms)
    {
        tracing::warn!(event = "bus.report.failed", agent_id = report.agent.0, %error,
            "Orchestrator report not shown in MASTER");
    }
}

#[cfg(test)]
#[path = "tests/reports.rs"]
mod tests;
