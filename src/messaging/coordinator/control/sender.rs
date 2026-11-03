//! Resolve callers before sending, replying or applying the MASTER guard.
use super::{
    AgentId, Author, ControlRequest, Method, PromptId, RequestId, Response, ResponseResult, RoomId,
    Worker,
};
use crate::messaging::control::SendCaller;
use serde_json::Value;

const UNKNOWN_CALLER: &str = "Cannot determine the send caller; pass --as <your name or id>";

impl Worker {
    pub(super) fn send_author(
        &mut self,
        room: RoomId,
        p: &Value,
    ) -> Result<Option<AgentId>, String> {
        if let Some(selector) = p.get("as") {
            return self
                .control_author(
                    room,
                    selector
                        .as_str()
                        .ok_or("Author must be an agent name or ID")?,
                )
                .map(Some);
        }
        let Some(caller) = p.get("caller") else {
            // Trusted in-process submissions have no wire context. The control
            // server supplies null for legacy anonymous clients, which is refused.
            return Ok(None);
        };
        let caller: SendCaller =
            serde_json::from_value(caller.clone()).map_err(|_| UNKNOWN_CALLER.to_owned())?;
        if caller.interrupt_request_id.is_some() {
            return self
                .developer_interrupt_follow_up(room, p, &caller)
                .then_some(None)
                .ok_or_else(|| UNKNOWN_CALLER.to_owned());
        }
        if caller.launch_id.is_some() {
            let (agent, ambiguous) = {
                let mut candidates = self.state.agents().filter(|agent| {
                    !agent.deletion_pending
                        && caller.launch_id.as_ref().is_none_or(|launch| {
                            agent.runtime_identity.launch_id.as_ref() == Some(launch)
                        })
                        && caller.pane_id.as_ref().is_none_or(|pane| {
                            agent.runtime_identity.pane_id.as_ref() == Some(pane)
                        })
                });
                (
                    candidates.next().map(|agent| agent.id),
                    candidates.next().is_some(),
                )
            };
            if ambiguous {
                return Err(UNKNOWN_CALLER.into());
            }
            if let Some(agent) = agent {
                // Inherited hints survive provider exit. The native pane must
                // still be owned by this same live agent incarnation.
                let author = self.process_author(&caller)?;
                return (author == Some(agent))
                    .then_some(author)
                    .ok_or_else(|| UNKNOWN_CALLER.to_owned());
            }
            // A stale launch cannot acquire a replacement's pane identity.
            return Err(UNKNOWN_CALLER.into());
        }
        self.process_author(&caller)
    }

    /// Only the internal follow-up of a successful developer interrupt can
    /// reuse its actor. Missing/failed receipts and other targets never qualify.
    fn developer_interrupt_follow_up(&self, room: RoomId, p: &Value, caller: &SendCaller) -> bool {
        if caller.launch_id.is_some()
            || caller.pane_id.is_some()
            || caller.process.is_some()
            || caller.agent_context
            || caller.bus_context
        {
            return false;
        }
        let Some(receipt) = caller
            .interrupt_request_id
            .as_ref()
            .and_then(|id| self.control_receipts.get(id))
            .filter(|r| r.request.method == "agent.interrupt" && r.response.ok)
        else {
            return false;
        };
        let result = &receipt.response.result;
        if result.get("author_id") != Some(&Value::Null)
            || result
                .get("conversation_room_id")
                .or_else(|| result.get("room_id"))
                .and_then(Value::as_u64)
                != Some(room.0)
        {
            return false;
        }
        let Some(targets) = p.get("to").and_then(Value::as_array) else {
            return false;
        };
        let Some(rows) = result.get("agents").and_then(Value::as_array) else {
            return false;
        };
        targets.iter().all(|target| {
            rows.iter().any(|row| {
                target
                    .as_str()
                    .zip(row["agent_id"].as_u64())
                    .is_some_and(|(target, id)| target == id.to_string())
                    && matches!(
                        row["outcome"].as_str(),
                        Some("already_idle" | "interrupting" | "stopped")
                    )
            })
        })
    }

    fn process_author(&mut self, caller: &SendCaller) -> Result<Option<AgentId>, String> {
        let process = caller.process.ok_or(UNKNOWN_CALLER)?;
        let location = match self.transport.request(Method::PaneLocateProcess(process)) {
            Ok(ResponseResult::CallerLocation { location }) => location,
            Err(error) => {
                tracing::debug!(event = "bus.send.caller_lookup_failed",
                    error_code = ?error.code, detail = %error.message,
                    caller_pid = process.pid, "Native caller lookup refused");
                return Err(UNKNOWN_CALLER.into());
            }
            Ok(_) => {
                tracing::debug!(
                    event = "bus.send.caller_lookup_failed",
                    error_code = "unexpected_response",
                    caller_pid = process.pid,
                    "Native caller lookup returned an unexpected response"
                );
                return Err(UNKNOWN_CALLER.into());
            }
        };
        let Some(pane) = location.pane else {
            return if !location.agent_owned
                && !caller.agent_context
                && !caller.bus_context
                && caller.pane_id.is_none()
            {
                Ok(None)
            } else {
                Err(UNKNOWN_CALLER.into())
            };
        };
        if caller
            .pane_id
            .as_ref()
            .is_some_and(|hint| *hint != pane.pane_id)
        {
            return Err(UNKNOWN_CALLER.into());
        }
        let mut candidates = self.state.agents().filter(|agent| {
            agent.runtime_identity.pane_id.as_ref() == Some(&pane.pane_id)
                && agent.runtime_identity.terminal_id.as_ref() == Some(&pane.terminal_id)
        });
        if let Some(agent) = candidates.next() {
            return if agent.deletion_pending
                || candidates.next().is_some()
                || !location.agent_owned
                || pane.agent_process_exited
            {
                Err(UNKNOWN_CALLER.into())
            } else {
                Ok(Some(agent.id))
            };
        }
        if !location.agent_owned
            && !caller.agent_context
            && pane.agent.is_none()
            && pane.agent_session.is_none()
        {
            Ok(None)
        } else {
            Err(UNKNOWN_CALLER.into())
        }
    }
}

impl Worker {
    /// Process metadata changes between CLI retries; compare the resolved actor
    /// as well as the business parameters before replaying an inferred send.
    pub(super) fn replay_control_receipt(&mut self, request: &ControlRequest) -> Response {
        let receipt = &self.control_receipts[&request.id];
        if receipt.request.method == request.method && receipt.request.params == request.params {
            return receipt.response.clone();
        }
        let mut previous = receipt.request.params.clone();
        let mut current = request.params.clone();
        let inferred_sender = receipt.inferred_sender.clone();
        let cached = receipt.response.clone();
        if request.method == "message.send"
            && receipt.request.method == request.method
            && current.get("as").is_none()
            && previous.get("as").is_none()
            && current.get("caller").is_some()
            && previous.get("caller").is_some()
        {
            if let (Some(previous), Some(current)) =
                (previous.as_object_mut(), current.as_object_mut())
            {
                previous.remove("caller");
                current.remove("caller");
            }
            if previous == current {
                if let Some(expected) = inferred_sender {
                    // No explicit author uses this room; the actor is global.
                    let room = self.state.master_room().map_or(RoomId(0), |room| room.id);
                    match self.send_author(room, &request.params) {
                        Ok(author)
                            if author.map_or(Author::Developer, Author::Agent) == expected =>
                        {
                            return cached;
                        }
                        Err(error) => {
                            return Response::failure(&request.id, "command_failed", error)
                        }
                        _ => {}
                    }
                }
            }
        }
        Response::failure(
            &request.id,
            "id_conflict",
            "Request ID already used with different parameters",
        )
    }

    pub(super) fn receipt_sender(
        &self,
        request: &ControlRequest,
        response: &Response,
    ) -> Option<Author> {
        if request.method != "message.send"
            || request.params.get("as").is_some()
            || request.params.get("caller").is_none()
            || !response.ok
        {
            return None;
        }
        if response.result["stage"] == "replied" {
            let id = RequestId(response.result["request_ids"][0].as_u64()?);
            return self
                .state
                .request(id)
                .map(|request| Author::Agent(request.agent_id));
        }
        let id = PromptId(response.result["message_id"].as_u64()?);
        self.state
            .requests_for_message(id)
            .next()
            .map(|request| request.prompt.author.clone())
            .or_else(|| self.state.notice(id).map(|notice| notice.author.clone()))
    }
}
