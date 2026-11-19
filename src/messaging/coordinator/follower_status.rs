//! Pending status hints for opted-in async send followers.
use super::{
    AgentId, Arc, Author, BTreeMap, BusSnapshot, BusState, Mutex, PromptId, RequestPhase, Worker,
};
use crate::messaging::control::{Request, Response};
use serde_json::{json, Value};
use std::{sync::Weak, time::Instant};

type WaiterLeases = Mutex<BTreeMap<PromptId, Instant>>;
pub(super) type ReplyWaiters = Arc<WaiterLeases>;

#[derive(Clone)]
pub(crate) struct FollowerStatus {
    snapshots: Arc<Mutex<Arc<BusSnapshot>>>,
    waiters: Weak<WaiterLeases>,
}

impl FollowerStatus {
    pub(super) fn new(snapshots: Arc<Mutex<Arc<BusSnapshot>>>, worker: &Worker) -> Self {
        Self {
            snapshots,
            waiters: Arc::downgrade(&worker.reply_waiters),
        }
    }

    /// Only a pending hint can come from the last published snapshot. The
    /// coordinator still decides outcomes, reconciles dialogs and expires
    /// queued requests. The follower discards pending hints and polls again.
    pub(crate) fn response(&self, request: &Request) -> Option<Response> {
        let params = request.params.as_object()?;
        if request.method != "message.status"
            || request.id.len() > 128
            || params
                .keys()
                .any(|key| !matches!(key.as_str(), "message" | "as"))
        {
            return None;
        }
        let message = PromptId(params.get("message")?.as_str()?.parse().ok()?);
        // A retained UI snapshot must not keep serving a stopped coordinator.
        let waiters = self.waiters.upgrade()?;
        let snapshot = Arc::clone(&*self.snapshots.try_lock().ok()?);
        let state = &snapshot.state;
        let mut recipients = state.requests_for_message(message).peekable();
        let author = &recipients.peek()?.prompt.author;
        if !recipients.all(|recipient| {
            matches!(
                recipient.phase,
                RequestPhase::Active | RequestPhase::Submitting
            ) && !recipient.settled()
                && state.agent(recipient.agent_id).is_some_and(|agent| {
                    !agent.dialog && !agent.deletion_pending && agent.delivery_error().is_none()
                })
        }) {
            return None;
        }
        if let Some(selector) = params.get("as") {
            if !is_author(state, author, selector) {
                return None;
            }
            // Reply routing reads this same map. Acknowledging a pending poll
            // before recording its lease could create a duplicate wake.
            waiters.try_lock().ok()?.insert(message, Instant::now());
        }
        Some(Response::success(
            &request.id,
            json!({"message_id": message, "complete": false, "requests": [], "snapshot_pending": true}),
        ))
    }
}

fn is_author(state: &BusState, author: &Author, selector: &Value) -> bool {
    let (Author::Agent(id), Some(selector)) = (author, selector.as_str()) else {
        return false;
    };
    let Some(agent) = state.agent(*id).filter(|agent| !agent.deletion_pending) else {
        return false;
    };
    if let Ok(raw) = selector.parse::<u64>() {
        let exact = AgentId(raw);
        if state.agent(exact).is_some() {
            return exact == *id;
        }
        if state.retired_agent(exact).is_some() {
            return false;
        }
    }
    agent.name == selector
        && state
            .agents()
            .filter(|agent| agent.name == selector)
            .count()
            == 1
}

#[cfg(all(test, unix))]
#[path = "tests/follower_status_test.rs"]
mod tests;
