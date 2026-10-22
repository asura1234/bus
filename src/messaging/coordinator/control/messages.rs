//! Messages development commands on the single coordinator.
use super::{
    mpsc, optional_text, required, AgentId, AgentRecipients, Author, BusEvent, Draft, ModelError,
    PathBuf, PromptId, Request, RequestId, RequestPhase, RoomId, RoomKind, Worker,
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
            "message.send" => self.control_send(p),
            "message.status" => {
                let id = required(p, "message")?
                    .parse::<u64>()
                    .map_err(|_| "Message must be a numeric ID")?;
                let message = PromptId(id);
                let result = self.control_message(message)?;
                if let Some(selector) = optional_text(p, "as")? {
                    if let Some(source) = self.state.requests().find(|r| r.prompt.id == message) {
                        let author = self.control_author(source.room_id, selector)?;
                        if self.state.reply_orchestrator(source) == Some(author) {
                            self.reply_waiters
                                .insert(message, std::time::Instant::now());
                        }
                    }
                }
                Ok(result)
            }
            "message.seen" => self.control_seen(p),
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
                            "Typed request recovery requires an idle agent or an unavailable agent with an invalidated session".into()
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

    /// Spend only components present in the flushed result, never a final
    /// that arrived between that output and its acknowledgement.
    fn control_seen(&mut self, p: &Value) -> Result<Value, String> {
        let request = RequestId(
            required(p, "request")?
                .parse::<u64>()
                .map_err(|_| "Request must be a numeric ID")?,
        );
        let room = self
            .state
            .request(request)
            .ok_or("Unknown request ID")?
            .room_id;
        let author = self.control_author(room, required(p, "as")?)?;
        let reply = match p.get("reply") {
            Some(Value::Bool(reply)) => *reply,
            Some(_) => return Err("reply must be a boolean".into()),
            None => p.get("events").is_none(),
        };
        let events = p
            .get("events")
            .map(|value| {
                serde_json::from_value::<Vec<String>>(value.clone())
                    .map_err(|_| "events must be an array of event keys")
            })
            .transpose()?;
        if !reply && events.is_none() {
            return Err("No printed reply or event selected".into());
        }
        let now = crate::messaging::storage::io::now_ms();
        let mut state = self.state.clone();
        if reply {
            state
                .see_worker_reply(request, author, now)
                .map_err(|e| e.to_string())?;
        }
        if let Some(keys) = events {
            state
                .see_worker_outcomes(request, author, &keys, now)
                .map_err(|e| e.to_string())?;
        }
        self.save(state)?;
        Ok(json!({"request_id":request,"seen":true}))
    }

    pub(in crate::messaging::coordinator) fn control_send(
        &mut self,
        p: &Value,
    ) -> Result<Value, String> {
        let room = self.control_room(required(p, "room")?)?;
        let author = match p.get("as") {
            None => None,
            Some(selector) => Some(
                self.control_author(
                    room,
                    selector
                        .as_str()
                        .ok_or("Author must be an agent name or ID")?,
                )?,
            ),
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
        let to_developer = selectors
            .iter()
            .any(|s| s.eq_ignore_ascii_case(crate::messaging::model::DEVELOPER_RECIPIENT));
        if to_developer {
            return self.control_send_to_developer(p, room, author, selectors.len(), files);
        }
        let broadcast = selectors == ["all"] || selectors.len() > 1;
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
            let sender_room = author
                .and_then(|id| self.state.agent(id))
                .map_or(room, |agent| agent.room_id);
            let recipients = selectors
                .into_iter()
                .map(|s| self.control_message_agent(room, sender_room, s, true))
                .collect::<Result<AgentRecipients, _>>()?;
            if author.is_some_and(|author| recipients.contains(&author)) {
                return Err("An agent cannot send a message to itself".into());
            }
            recipients
        };
        let mut state = self.state.clone();
        let ids = state
            .submit_message_with_broadcast(
                room,
                Draft {
                    text: optional_text(p, "text")?.unwrap_or_default().into(),
                    files,
                    recipient_ids: recipients,
                },
                author.map_or(Author::Developer, Author::Agent),
                crate::messaging::storage::io::now_ms(),
                p.get("queue").and_then(Value::as_bool).unwrap_or(false),
                broadcast,
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

    /// Exact IDs are global; names prefer the conversation, then sender's room. The reserved
    /// alias belongs only to `--to`, after real local names have had precedence.
    fn control_message_agent(
        &self,
        room: RoomId,
        sender_room: RoomId,
        selector: &str,
        recipient_alias: bool,
    ) -> Result<AgentId, String> {
        if let Some(agent) = selector
            .parse::<u64>()
            .ok()
            .and_then(|id| self.state.agent(AgentId(id)))
        {
            return Ok(agent.id);
        }
        let mut candidates: Vec<_> = self.state.agents().filter(|a| a.name == selector).collect();
        if let Some(scope) = [room, sender_room]
            .into_iter()
            .find(|scope| candidates.iter().any(|a| a.room_id == *scope))
        {
            candidates.retain(|a| a.room_id == scope);
        } else if recipient_alias && selector.eq_ignore_ascii_case("orchestrator") {
            return self
                .state
                .orchestrator_of(sender_room)
                .map(|a| a.id)
                .ok_or_else(|| "This room has no bound orchestrator".into());
        }
        match candidates.as_slice() {
            [agent] => Ok(agent.id),
            [] => Err("No matching room or agent".into()),
            _ => Err(format!(
                "Ambiguous agent name {selector:?}; use an ID: {}",
                candidates
                    .iter()
                    .map(|a| format!("{} (agent {}, room {})", a.name, a.id.0, a.room_id.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    pub(in crate::messaging::coordinator) fn control_author(
        &self,
        room: RoomId,
        selector: &str,
    ) -> Result<AgentId, String> {
        let id = self.control_message_agent(room, room, selector, false)?;
        if self.state.agent(id).is_some_and(|a| a.deletion_pending) {
            return Err("Author agent is being deleted".into());
        }
        Ok(id)
    }

    /// `send --to human`: an agent's message to the developer in MASTER. It is
    /// delivered to no agent, so it has no requests and nothing to wait for.
    pub(in crate::messaging::coordinator) fn control_send_to_developer(
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
        let author = author
            .ok_or("--to human requires --as AGENT: the developer cannot message themselves")?;
        // Reports belong where the developer reads them all: one MASTER chat.
        if self.state.room(room).map(|r| r.kind) != Some(RoomKind::Master) {
            return Err("--to human posts only in the MASTER room; use --room master".into());
        }
        if p.get("queue").and_then(Value::as_bool).unwrap_or(false) {
            return Err("--queue does not apply to --to human".into());
        }
        let mut state = self.state.clone();
        let message = state
            .post_to_developer(
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

    pub(in crate::messaging::coordinator) fn control_message(
        &mut self,
        message: PromptId,
    ) -> Result<Value, String> {
        let now_ms = crate::messaging::storage::io::now_ms();
        // Poll metadata can lag a dialog closing. Confirm flagged recipients
        // from their current screen without opening or answering any question.
        let agents: std::collections::BTreeSet<_> = self
            .state
            .requests()
            .filter(|request| {
                request.prompt.id == message
                    && !matches!(
                        request.phase,
                        RequestPhase::Completed | RequestPhase::Abandoned
                    )
            })
            .filter_map(|request| self.state.agent(request.agent_id))
            .filter(|agent| agent.dialog)
            .map(|agent| agent.id)
            .collect();
        // One write for every observation; `save` skips it when none changed.
        if !agents.is_empty() {
            self.observe_recipient_dialogs(agents)?;
        }
        self.expire_queued_requests(now_ms)?;
        self.control_message_at(message, now_ms)
    }

    /// Re-reads each flagged recipient's screen, without opening or answering
    /// any question, and records what it shows.
    fn observe_recipient_dialogs(
        &mut self,
        agents: std::collections::BTreeSet<AgentId>,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        for agent in agents {
            let observation = (|| {
                let (_, _, terminal, pane, session) = self.dialog_identity(agent)?;
                self.native_dialog(&pane, &terminal, session.as_deref(), false)
            })();
            let Ok(observation) = observation else {
                // An unreadable screen cannot prove the cached dialog closed.
                continue;
            };
            state
                .observe_dialog(
                    agent,
                    observation.dialog.is_some() || observation.pending_question,
                    crate::messaging::storage::io::now_ms(),
                )
                .map_err(|error| error.to_string())?;
        }
        self.save(state)
    }

    /// `message status` as of `now_ms`, which stall detection measures against.
    pub(in crate::messaging::coordinator) fn control_message_at(
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
            let outcomes = self.state.worker_outcomes_for(r);
            let worker_events = self.state.worker_events_for(r);
            let phase_stage = match r.phase { RequestPhase::Queued=>"queued",RequestPhase::Submitting=>"submitting",RequestPhase::Active if r.group.is_some()=>"joined",RequestPhase::Active if r.trusted_start_bound=>"delivered",RequestPhase::Active=>"awaiting_start",RequestPhase::Completed=>"replied",RequestPhase::Abandoned=>"abandoned" };
            // A stalled request reports why instead of its phase; `stalled_from`
            // keeps the phase it stalled in.
            let stall = self.state.stall_reason(r, now_ms);
            let stage = if stall.is_some() { "stalled" } else { phase_stage };
            // The native server's last refusal explains a queued wait best.
            let reason = stall
                .or_else(|| r.failure_reason.clone())
                .or_else(|| self.state.blocked_unanswered(r, now_ms).then(|| "blocked_unanswered".into()))
                .or_else(|| (r.phase == RequestPhase::Queued).then(|| agent.and_then(|a| a.delivery_rejection.clone().or_else(|| crate::messaging::diagnostics::wait_reason(a).map(Into::into)))).flatten());
            json!({"request_id":r.id,"agent_id":r.agent_id,"agent_name":agent.map(|a|&a.name),"stage":stage,"stalled_from":(stage=="stalled").then_some(phase_stage),"reason":reason,"status":agent.map(|a|a.status),"uncertain_outcome":r.uncertain_outcome,"session_id":r.provider_session_id,"turn_id":r.provider_turn_id,"start_bound":r.trusted_start_bound,"dialog":self.waiting_on_dialog(r),"turn_ended":self.state.turn_ended(r),"settling":self.state.settling(r),"group":r.group,"queue":r.queue_only,"event_acks":outcomes.iter().map(|(key,_)|key).collect::<Vec<_>>(),"events":worker_events.iter().map(|(key,text)|json!({"key":key,"text":text})).collect::<Vec<_>>(),"reply_ack":self.state.reply_needs_ack(r),"exit_snapshot":agent.and_then(|a|a.exit_snapshot.as_ref()),"reply":if r.phase==RequestPhase::Completed {r.pending_final.as_ref()}else{None}})
        }).collect::<Vec<_>>()}),
        )
    }

    /// An unsettled request whose agent shows a readable or pending dialog.
    pub(in crate::messaging::coordinator) fn waiting_on_dialog(&self, request: &Request) -> bool {
        !matches!(
            request.phase,
            RequestPhase::Completed | RequestPhase::Abandoned
        ) && self
            .state
            .agent(request.agent_id)
            .is_some_and(|agent| agent.dialog)
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
