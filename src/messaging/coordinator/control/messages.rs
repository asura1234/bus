//! Messages development commands on the single coordinator.
use super::{
    mpsc, optional_text, required, AgentId, AgentRecipients, Author, BusEvent, Draft, ModelError,
    PathBuf, PromptId, Request, RequestId, RequestPhase, RoomId, RoomKind, RuntimeStatus, Worker,
};
use serde_json::{json, Value};

impl Worker {
    pub(super) fn execute_messages(
        &mut self,
        method: &str,
        p: &Value,
        _events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<Value, String> {
        match method {
            "message.send" => self.dev_send(p),
            "message.status" => {
                let id = required(p, "message")?
                    .parse::<u64>()
                    .map_err(|_| "Message must be a numeric ID")?;
                self.dev_message(PromptId(id))
            }
            "request.recover" => {
                let id = required(p, "request")?
                    .parse::<u64>()
                    .map_err(|_| "Request must be a numeric ID")?;
                let request = RequestId(id);
                let agent = self
                    .state
                    .request(request)
                    .ok_or("Unknown request ID")?
                    .agent_id;
                let mut state = self.state.clone();
                state
                    .recover_idle_request(request, crate::messaging::storage::io::now_ms())
                    .map_err(|error| match error {
                        ModelError::AgentNotIdle => {
                            "Request recovery is allowed only while its agent is idle".into()
                        }
                        _ => error.to_string(),
                    })?;
                self.save(state)?;
                crate::messaging::diagnostics::request(
                    &self.state,
                    request,
                    "bus.message.recovered",
                    "abandoned",
                );
                Ok(json!({"request_id":request,"agent_id":agent,"stage":"abandoned"}))
            }
            _ => Err("Unknown method".into()),
        }
    }

    pub(in crate::messaging::coordinator) fn dev_send(
        &mut self,
        p: &Value,
    ) -> Result<Value, String> {
        let room = self.dev_room(required(p, "room")?)?;
        let author = match p.get("as") {
            None => None,
            Some(selector) => {
                let selector = selector
                    .as_str()
                    .ok_or("Author must be an agent name or ID")?;
                // A MASTER orchestrator writes into the room it orchestrates
                // without being a member; no other cross-room author is allowed.
                // Fall back only when nothing in the room matches; an ambiguous
                // in-room name must fail rather than resolve to the orchestrator.
                let in_room = self.state.agents().any(|a| {
                    a.room_id == room && (selector == a.id.0.to_string() || selector == a.name)
                });
                let id = if in_room {
                    self.dev_agent(selector, Some(room))?
                } else {
                    self.state
                        .orchestrator_of(room)
                        .filter(|o| selector == o.id.0.to_string() || selector == o.name)
                        .map(|o| o.id)
                        .ok_or(
                            "No matching room or agent: --as must name an agent in this room or its orchestrator",
                        )?
                };
                if self.state.agent(id).is_some_and(|a| a.deletion_pending) {
                    return Err("Author agent is being deleted".into());
                }
                Some(id)
            }
        };
        let selected = p
            .get("to")
            .and_then(Value::as_array)
            .ok_or("Recipients must be an explicit array")?;
        let selectors = selected
            .iter()
            .map(|v| v.as_str().ok_or("Recipient must be a name or ID"))
            .collect::<Result<Vec<_>, _>>()?;
        let files = attachment_files(p)?;
        let to_human = selectors
            .iter()
            .any(|s| s.eq_ignore_ascii_case(crate::messaging::model::HUMAN_RECIPIENT));
        if to_human {
            return self.dev_send_to_human(p, room, author, selectors.len(), files);
        }
        let recipients: AgentRecipients = if selectors == ["all"] {
            // An agent's broadcast goes to everyone else in the room.
            let recipients: AgentRecipients = self
                .state
                .agents()
                .filter(|a| a.room_id == room && Some(a.id) != author)
                .map(|a| a.id)
                .collect();
            if recipients.is_empty() {
                return Err(if author.is_some() {
                    "No recipients besides the author: the room has no other agents".into()
                } else {
                    "No recipients: the room has no agents".into()
                });
            }
            recipients
        } else {
            let recipients = selectors
                .into_iter()
                .map(|s| self.dev_agent(s, Some(room)))
                .collect::<Result<AgentRecipients, _>>()?;
            if author.is_some_and(|author| recipients.contains(&author)) {
                return Err("An agent cannot send a message to itself".into());
            }
            recipients
        };
        let mut state = self.state.clone();
        let ids = state
            .submit_message_with(
                room,
                Draft {
                    text: optional_text(p, "text")?.unwrap_or_default().into(),
                    files,
                    recipient_ids: recipients,
                },
                author.map_or(Author::Human, Author::Agent),
                crate::messaging::storage::io::now_ms(),
                p.get("queue").and_then(Value::as_bool).unwrap_or(false),
            )
            .map_err(|e| e.to_string())?;
        let message = state
            .request(ids[0])
            .ok_or("Submitted request missing")?
            .prompt
            .id;
        self.save(state)?;
        for id in &ids {
            crate::messaging::diagnostics::request(
                &self.state,
                *id,
                "bus.message.queued",
                "persisted",
            );
        }
        Ok(json!({"message_id":message,"request_ids":ids,"stage":"queued"}))
    }

    /// `send --to human`: an agent's message to the Human in MASTER. It is
    /// delivered to no agent, so it has no requests and nothing to wait for.
    pub(in crate::messaging::coordinator) fn dev_send_to_human(
        &mut self,
        p: &Value,
        room: RoomId,
        author: Option<AgentId>,
        selector_count: usize,
        files: Vec<PathBuf>,
    ) -> Result<Value, String> {
        if selector_count != 1 {
            return Err("--to human cannot be combined with agent recipients".into());
        }
        let author =
            author.ok_or("--to human requires --as AGENT: the human cannot message themselves")?;
        // Reports belong where the Human reads them all: one MASTER chat.
        if self.state.room(room).map(|r| r.kind) != Some(RoomKind::Master) {
            return Err("--to human posts only in the MASTER room; use --room master".into());
        }
        if p.get("queue").and_then(Value::as_bool).unwrap_or(false) {
            return Err("--queue does not apply to --to human".into());
        }
        let mut state = self.state.clone();
        let message = state
            .post_to_human(
                room,
                author,
                optional_text(p, "text")?.unwrap_or_default().into(),
                files,
                crate::messaging::storage::io::now_ms(),
            )
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Ok(json!({"message_id":message,"request_ids":[],"stage":"posted"}))
    }

    pub(in crate::messaging::coordinator) fn dev_message(
        &self,
        message: PromptId,
    ) -> Result<Value, String> {
        self.dev_message_at(message, crate::messaging::storage::io::now_ms())
    }

    /// `message status` as of `now_ms`, which stall detection measures against.
    pub(in crate::messaging::coordinator) fn dev_message_at(
        &self,
        message: PromptId,
        now_ms: u64,
    ) -> Result<Value, String> {
        let mut requests = self
            .state
            .requests()
            .filter(|r| r.prompt.id == message)
            .collect::<Vec<_>>();
        if requests.is_empty() {
            return Err("Unknown message ID".into());
        }
        let recipient_order = &requests[0].prompt.recipient_ids;
        requests.sort_by_key(|request| {
            recipient_order
                .iter()
                .position(|id| *id == request.agent_id)
                .unwrap_or(usize::MAX)
        });
        Ok(
            json!({"message_id":message,"files":requests[0].prompt.files,"complete":requests.iter().all(|r|matches!(r.phase,RequestPhase::Completed|RequestPhase::Abandoned)),"waiting_on_dialog":requests.iter().filter(|r| self.waiting_on_dialog(r)).map(|r| r.agent_id).collect::<Vec<_>>(),"requests":requests.iter().map(|r| {
            let agent = self.state.agent(r.agent_id);
            let phase_stage = match r.phase { RequestPhase::Queued=>"queued",RequestPhase::Submitting=>"submitting",RequestPhase::Active if r.group.is_some()=>"joined",RequestPhase::Active if r.trusted_start_bound=>"delivered",RequestPhase::Active=>"awaiting_start",RequestPhase::Completed=>"replied",RequestPhase::Abandoned=>"abandoned" };
            // A stalled request reports why instead of its phase; `stalled_from`
            // keeps the phase it stalled in.
            let stall = self.state.stall_reason(r, now_ms);
            let stage = if stall.is_some() { "stalled" } else { phase_stage };
            // The native server's last refusal explains a queued wait best.
            let reason = stall
                .or_else(|| self.state.blocked_unanswered(r, now_ms).then(|| "blocked_unanswered".into()))
                .or_else(|| (r.phase == RequestPhase::Queued).then(|| agent.and_then(|a| a.delivery_rejection.clone().or_else(|| crate::messaging::diagnostics::wait_reason(a).map(Into::into)))).flatten());
            json!({"request_id":r.id,"agent_id":r.agent_id,"agent_name":agent.map(|a|&a.name),"stage":stage,"stalled_from":(stage=="stalled").then_some(phase_stage),"reason":reason,"status":agent.map(|a|a.status),"uncertain_outcome":r.uncertain_outcome,"session_id":r.provider_session_id,"turn_id":r.provider_turn_id,"start_bound":r.trusted_start_bound,"dialog":self.waiting_on_dialog(r),"turn_ended":self.state.turn_ended(r),"group":r.group,"queue":r.queue_only,"reply":if r.phase==RequestPhase::Completed {r.pending_final.as_ref()}else{None}})
        }).collect::<Vec<_>>()}),
        )
    }

    /// An unsettled request whose agent shows a dialog or a blocked screen.
    pub(in crate::messaging::coordinator) fn waiting_on_dialog(&self, request: &Request) -> bool {
        !matches!(
            request.phase,
            RequestPhase::Completed | RequestPhase::Abandoned
        ) && self
            .state
            .agent(request.agent_id)
            .is_some_and(|agent| agent.dialog || agent.status == RuntimeStatus::Blocked)
    }
}

/// The validated, deduplicated `files` of a send.
fn attachment_files(p: &Value) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    if let Some(value) = p.get("files") {
        let home = std::env::home_dir().ok_or("Home directory unavailable")?;
        for item in value.as_array().ok_or("Files must be an array")? {
            let path = crate::messaging::attachments::validate_attachment(
                item.as_str().ok_or("File must be a path")?,
                &home,
            )
            .map_err(|e| e.to_string())?;
            if !files.contains(&path) {
                files.push(path);
            }
        }
    }
    Ok(files)
}
