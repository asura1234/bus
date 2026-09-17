//! Dev commands execute on the existing single-writer coordinator.
use super::*;
use crate::bus::control::{Request as ControlRequest, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PermissionFingerprintClaims {
    room_id: RoomId,
    agent_id: AgentId,
    participant_incarnation: u64,
    launch_id: String,
    terminal_id: String,
    session_id: String,
    pane_id: String,
    current_request: RequestId,
    provider_turn: String,
    content_revision: u64,
    prompt_digest: String,
}

pub(super) struct PermissionCandidate {
    claims: PermissionFingerprintClaims,
    native: schema::AgentPermissionObservation,
    fingerprint: String,
}

impl Worker {
    #[cfg(test)]
    fn dev_response(&mut self, request: &ControlRequest) -> Response {
        self.dev_response_with_events(request, None)
    }

    pub(super) fn dev_response_with_events(
        &mut self,
        request: &ControlRequest,
        events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Response {
        if !self.dev_enabled {
            return Response::failure(
                &request.id,
                "dev_disabled",
                "Start Bus with --dev to enable control",
            );
        }
        if request.id.is_empty() || request.id.len() > 128 {
            return Response::failure(
                &request.id,
                "invalid_request",
                "Request ID must contain 1–128 bytes",
            );
        }
        if let Some((prior, response)) = self.dev_receipts.get(&request.id) {
            return if prior.method == request.method && prior.params == request.params {
                response.clone()
            } else {
                Response::failure(
                    &request.id,
                    "id_conflict",
                    "Request ID already used with different parameters",
                )
            };
        }
        let (fields, mutation): (&[&str], bool) = match request.method.as_str() {
            "state" | "diagnostics" => (&[], false),
            "room.create" => (&["name"], true),
            "room.rename" => (&["room", "name"], true),
            "room.delete" => (&["room", "confirm"], true),
            "room.focus" => (&["room"], true),
            "agent.add" => (
                &[
                    "room",
                    "name",
                    "provider",
                    "cwd",
                    "extra_args",
                    "consent_project_hooks",
                ],
                true,
            ),
            "agent.delete" | "agent.setup-confirm" => (&["agent", "confirm"], true),
            "agent.read" => (&["agent", "source", "lines"], false),
            "agent.permission.observe" => (&["agent"], false),
            "agent.permission.approve_once" => (&["agent", "fingerprint", "response"], true),
            "agent.focus" => (&["agent"], true),
            "message.send" => (&["room", "to", "text", "files"], true),
            "message.status" => (&["message"], false),
            "request.recover" => (&["request", "confirm"], true),
            "room.history" => (&["room"], false),
            _ => {
                return Response::failure(
                    &request.id,
                    "unknown_method",
                    "Unknown Bus control method",
                )
            }
        };
        let Some(params) = request.params.as_object() else {
            return Response::failure(
                &request.id,
                "invalid_params",
                "Parameters must be an object",
            );
        };
        if params.keys().any(|key| !fields.contains(&key.as_str())) {
            return Response::failure(&request.id, "invalid_params", "Unknown parameter");
        }
        if (request.method.ends_with(".delete")
            || request.method == "agent.setup-confirm"
            || request.method == "request.recover")
            && request.params.get("confirm") != Some(&Value::Bool(true))
        {
            return Response::failure(
                &request.id,
                "confirmation_required",
                "Explicit --confirm is required",
            );
        }
        if mutation && self.storage_failed {
            return Response::failure(
                &request.id,
                "storage_unavailable",
                "Fix Bus storage and restart before making changes",
            );
        }
        // Never silently evict mutation receipts: an old retry must not launch or send twice.
        let reserve = serde_json::to_vec(request)
            .map_or(usize::MAX, |v| v.len())
            .saturating_add(4096);
        if mutation
            && (self.dev_receipts.len() >= 256
                || self.dev_receipt_bytes.saturating_add(reserve) > 8 * 1024 * 1024)
        {
            return Response::failure(
                &request.id,
                "receipt_capacity",
                "Dev mutation receipt limit reached; restart when safe",
            );
        }
        let _span = tracing::info_span!("bus.dev.command", control_request_id = %request.id, method = %request.method).entered();
        let started = std::time::Instant::now();
        let response = match self.dev_execute(&request.method, &request.params, events) {
            Ok(result) => Response::success(&request.id, result),
            Err(error) => {
                // Detailed domain diagnostics remain available via state and log IDs.
                Response::failure(
                    &request.id,
                    "command_failed",
                    error.chars().take(700).collect::<String>(),
                )
            }
        };
        tracing::info!(
            event = "bus.dev.command.result",
            ok = response.ok,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Bus dev command finished"
        );
        if mutation {
            self.dev_receipt_bytes = self.dev_receipt_bytes.saturating_add(reserve);
            self.dev_receipts
                .insert(request.id.clone(), (request.clone(), response.clone()));
        }
        response
    }

    fn dev_execute(
        &mut self,
        method: &str,
        p: &Value,
        events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<Value, String> {
        match method {
            "agent.focus" | "room.focus" => {
                let (room, agent) = if method == "agent.focus" {
                    let id = self.dev_agent(required(p, "agent")?, None)?;
                    let agent = self.state.agent(id).ok_or("Unknown agent")?;
                    if agent.runtime_identity.pane_id.is_none() {
                        return Err("Agent has no terminal; inspect its launch error".into());
                    }
                    (agent.room_id, Some(id))
                } else {
                    (self.dev_room(required(p, "room")?)?, None)
                };
                // Navigation is client presentation state. The UI uses its normal
                // focus command path; this receipt only attests enqueueing.
                events
                    .ok_or("Bus UI event channel unavailable")?
                    .send(BusEvent::DevFocusRequested { room, agent })
                    .map_err(|_| "Bus UI event channel disconnected")?;
                let mut result = json!({"stage":"queued", "room_id":room});
                if let Some(agent) = agent {
                    result["agent_id"] = json!(agent);
                }
                Ok(result)
            }
            "state" => Ok(
                json!({"revision":self.revision,"rooms":self.state.rooms().map(|r|json!({"id":r.id,"name":r.name,"notes":r.notes,"deletion_pending":r.deletion_pending})).collect::<Vec<_>>(),"agents":self.state.agents().collect::<Vec<_>>()}),
            ),
            "diagnostics" => Ok(
                json!({"version":env!("CARGO_PKG_VERSION"),"dev":true,"storage_failed":self.storage_failed,"coordinator_error":self.error,"data_dir":self.data_dir,"logs":self.data_dir.join("herdr-config/sessions/bus"),"callback_logs":self.data_dir.join("callbacks"),"agents":self.state.agents().map(|a|json!({"agent_id":a.id,"name":a.name,"status":a.status,"reason":crate::bus::diagnostics::wait_reason(a),"detail":a.actionable_error,"identity":a.runtime_identity,"current_request":a.current_request})).collect::<Vec<_>>()}),
            ),
            "room.create" => self.dev_command(BusCommand::CreateRoom(required(p, "name")?.into())),
            "room.rename" => self.dev_command(BusCommand::RenameRoom(
                self.dev_room(required(p, "room")?)?,
                required(p, "name")?.into(),
            )),
            "room.delete" => {
                self.dev_command(BusCommand::DeleteRoom(self.dev_room(required(p, "room")?)?))
            }
            "agent.delete" => self.dev_command(BusCommand::DeleteAgent(
                self.dev_agent(required(p, "agent")?, None)?,
            )),
            "agent.setup-confirm" => self.dev_command(BusCommand::CompleteHookSetup(
                self.dev_agent(required(p, "agent")?, None)?,
            )),
            "agent.add" => {
                let provider = match required(p, "provider")? {
                    "claude" => Provider::ClaudeCode,
                    "codex" => Provider::Codex,
                    "cursor" => Provider::Cursor,
                    _ => return Err("Provider must be claude, codex, or cursor".into()),
                };
                self.dev_command(BusCommand::AddAgent(AddAgent {
                    room: self.dev_room(required(p, "room")?)?,
                    name: required(p, "name")?.into(),
                    provider,
                    cwd: required(p, "cwd")?.into(),
                    extra_args: optional_text(p, "extra_args")?.unwrap_or_default().into(),
                    consent_project_hooks: optional_bool(p, "consent_project_hooks")?,
                }))
            }
            "agent.read" => self.dev_read(
                self.dev_agent(required(p, "agent")?, None)?,
                optional_text(p, "source")?,
                optional_u32(p, "lines")?,
            ),
            "agent.permission.observe" => {
                let agent = self.dev_agent(required(p, "agent")?, None)?;
                let candidate = self.permission_candidate(agent)?;
                Ok(permission_candidate_json(candidate))
            }
            "agent.permission.approve_once" => {
                let agent_id = self.dev_agent(required(p, "agent")?, None)?;
                if required(p, "response")? != "allow-once" {
                    return Err("Response must be allow-once".into());
                }
                self.approve_permission_once(
                    crate::bus::orchestrator::ParticipantId::Human,
                    Some(agent_id),
                    crate::bus::orchestrator::ExactPermissionGrant {
                        fingerprint: required(p, "fingerprint")?.into(),
                        response: crate::bus::orchestrator::ApprovedPermissionResponse::AllowOnce,
                    },
                )
            }
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
                    .recover_idle_request(request, crate::bus::io::now_ms())
                    .map_err(|error| match error {
                        ModelError::AgentNotIdle => {
                            "Request recovery is allowed only while its agent is idle".into()
                        }
                        _ => error.to_string(),
                    })?;
                self.save(state)?;
                crate::bus::diagnostics::request(
                    &self.state,
                    request,
                    "bus.message.recovered",
                    "abandoned",
                );
                Ok(json!({"request_id":request,"agent_id":agent,"stage":"abandoned"}))
            }
            "room.history" => {
                let room = self.dev_room(required(p, "room")?)?;
                let messages = self
                    .state
                    .requests()
                    .filter(|r| r.room_id == room)
                    .map(|r| (r.prompt.id, &r.prompt))
                    .collect::<BTreeMap<_, _>>();
                Ok(
                    json!({"room_id":room,"messages":messages.iter().map(|(id,prompt)|json!({"prompt":prompt,"delivery":self.dev_message(*id).ok()})).collect::<Vec<_>>()}),
                )
            }
            _ => Err("Unknown method".into()),
        }
    }

    fn dev_command(&mut self, command: BusCommand) -> Result<Value, String> {
        // Do not send navigation side effects to the human's TUI event queue.
        let (tx, rx) = mpsc::channel();
        self.command(command, &tx)?;
        for event in rx.try_iter() {
            match event {
                BusEvent::RoomCreated(id) => return Ok(json!({"room_id":id})),
                BusEvent::AgentAdded(id) => return Ok(json!({"agent_id":id,"stage":"launching"})),
                BusEvent::SetupRequired { notice, .. } => {
                    return Err(format!(
                        "Project hook setup requires --consent-hooks: {} ({})",
                        notice.message,
                        notice.path.display()
                    ))
                }
                _ => {}
            }
        }
        Ok(json!({"updated":true}))
    }

    fn dev_room(&self, selector: &str) -> Result<RoomId, String> {
        unique(
            self.state
                .rooms()
                .filter(|r| selector == r.id.0.to_string() || selector == r.name)
                .map(|r| r.id),
        )
    }

    fn dev_agent(&self, selector: &str, room: Option<RoomId>) -> Result<AgentId, String> {
        unique(
            self.state
                .agents()
                .filter(|a| {
                    room.is_none_or(|r| r == a.room_id)
                        && (selector == a.id.0.to_string() || selector == a.name)
                })
                .map(|a| a.id),
        )
    }

    fn dev_send(&mut self, p: &Value) -> Result<Value, String> {
        let room = self.dev_room(required(p, "room")?)?;
        let selected = p
            .get("to")
            .and_then(Value::as_array)
            .ok_or("Recipients must be an explicit array")?;
        let selectors = selected
            .iter()
            .map(|v| v.as_str().ok_or("Recipient must be a name or ID"))
            .collect::<Result<Vec<_>, _>>()?;
        let recipients = if selectors == ["all"] {
            self.state
                .agents()
                .filter(|a| a.room_id == room)
                .map(|a| a.id)
                .collect()
        } else {
            selectors
                .into_iter()
                .map(|s| self.dev_agent(s, Some(room)))
                .collect::<Result<AgentRecipients, _>>()?
        };
        let mut files = Vec::new();
        if let Some(value) = p.get("files") {
            let home = std::env::home_dir().ok_or("Home directory unavailable")?;
            for item in value.as_array().ok_or("Files must be an array")? {
                let path = crate::bus::files::validate_attachment(
                    item.as_str().ok_or("File must be a path")?,
                    &home,
                )
                .map_err(|e| e.to_string())?;
                if !files.contains(&path) {
                    files.push(path);
                }
            }
        }
        let mut state = self.state.clone();
        let ids = state
            .submit_message(
                room,
                Draft {
                    text: optional_text(p, "text")?.unwrap_or_default().into(),
                    files,
                    recipient_ids: recipients,
                },
                crate::bus::io::now_ms(),
            )
            .map_err(|e| e.to_string())?;
        let message = state
            .request(ids[0])
            .ok_or("Submitted request missing")?
            .prompt
            .id;
        self.save(state)?;
        for id in &ids {
            crate::bus::diagnostics::request(&self.state, *id, "bus.message.queued", "persisted");
        }
        let requests = ids
            .iter()
            .filter_map(|id| self.state.request(*id))
            .map(|request| self.dev_request_status(request))
            .collect::<Vec<_>>();
        Ok(json!({"message_id":message,"request_ids":ids,"requests":requests,"stage":"queued"}))
    }

    fn dev_message(&self, message: PromptId) -> Result<Value, String> {
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
        Ok(json!({
            "message_id":message,
            "complete":requests.iter().all(|r|matches!(r.phase,RequestPhase::Completed|RequestPhase::Abandoned)),
            "requests":requests.iter().map(|request| self.dev_request_status(request)).collect::<Vec<_>>()
        }))
    }

    fn dev_request_status(&self, request: &Request) -> Value {
        let agent = self.state.agent(request.agent_id);
        let stage = match request.phase {
            RequestPhase::Queued => "queued",
            RequestPhase::Submitting => "submitting",
            RequestPhase::Active if request.trusted_start_bound => "delivered",
            RequestPhase::Active => "awaiting_start",
            RequestPhase::Completed => "replied",
            RequestPhase::Abandoned => "abandoned",
        };
        let reason = match request.phase {
            RequestPhase::Queued => agent.and_then(crate::bus::diagnostics::wait_reason),
            RequestPhase::Abandoned => request.abandon_reason.map(RequestAbandonReason::as_str),
            _ => None,
        };
        let blocked_by_request_id = (reason == Some("prior_request_active"))
            .then(|| agent.and_then(|agent| agent.current_request))
            .flatten();
        json!({
            "request_id":request.id,
            "agent_id":request.agent_id,
            "agent_name":agent.map(|agent|&agent.name),
            "stage":stage,
            "reason":reason,
            "blocked_by_request_id":blocked_by_request_id,
            "detail":agent.and_then(|agent|agent.actionable_error.as_deref()),
            "status":agent.map(|agent|agent.status),
            "uncertain_outcome":request.uncertain_outcome,
            "session_id":request.provider_session_id,
            "turn_id":request.provider_turn_id,
            "start_bound":request.trusted_start_bound,
            "reply":if request.phase==RequestPhase::Completed {request.pending_final.as_ref()}else{None},
        })
    }

    pub(super) fn dev_read(
        &mut self,
        id: AgentId,
        source: Option<&str>,
        lines: Option<u32>,
    ) -> Result<Value, String> {
        let read_source = match source {
            Some("recent") => schema::ReadSource::Recent,
            Some("visible") => schema::ReadSource::Visible,
            None => return Err("Recent reads require an explicit positive lines value".into()),
            Some(_) => return Err("Source must be visible or recent".into()),
        };
        if read_source == schema::ReadSource::Visible && lines.is_some() {
            return Err("Visible reads return the complete viewport; omit lines".into());
        }
        if read_source == schema::ReadSource::Recent && lines.is_none_or(|lines| lines == 0) {
            return Err("Recent reads require an explicit positive lines value".into());
        }
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        let target = agent
            .runtime_identity
            .pane_id
            .clone()
            .ok_or("Agent has no terminal")?;
        let identity = agent.runtime_identity.clone();
        if identity.launch_id.is_none()
            || identity.terminal_id.is_none()
            || identity.pane_id.is_none()
            || identity.session_id.is_none()
        {
            return Err("Agent runtime identity is incomplete".into());
        }
        let expected_name = format!("bus-r{}-a{}", agent.room_id.0, id.0);
        let name = agent.name.clone();
        let status = agent.status;
        let current_request = agent.current_request;
        let response = self
            .transport
            .request(Method::AgentGet(schema::AgentTarget {
                target: target.clone(),
            }))
            .map_err(|e| e.message)?;
        let ResponseResult::AgentInfo { agent: info } = response else {
            return Err("Unexpected native agent response".into());
        };
        let identity_matches = |info: &schema::AgentInfo| {
            Some(&info.terminal_id) == identity.terminal_id.as_ref()
                && Some(&info.pane_id) == identity.pane_id.as_ref()
                && info.name.as_deref() == Some(&expected_name)
                && info
                    .agent_session
                    .as_ref()
                    .map(|value| value.value.as_str())
                    == identity.session_id.as_deref()
        };
        if !identity_matches(&info) {
            return Err("Agent terminal identity changed; inspect the owned session".into());
        }
        let native_lines = match read_source {
            schema::ReadSource::Visible => None,
            schema::ReadSource::Recent => lines,
            _ => return Err("Source must be visible or recent".into()),
        };
        let output = self
            .transport
            .request(Method::AgentRead(schema::AgentReadParams {
                target: target.clone(),
                source: read_source,
                lines: native_lines,
                format: schema::ReadFormat::Text,
                strip_ansi: true,
            }))
            .map_err(|e| e.message)?;
        let ResponseResult::PaneRead { read } = output else {
            return Err("Unexpected native read response".into());
        };
        if read.pane_id != target
            || read.source != read_source
            || read.requested_lines != native_lines
            || (read_source == schema::ReadSource::Visible
                && (read.viewport_rows.is_none() || read.viewport_columns.is_none()))
            || (read_source == schema::ReadSource::Recent
                && (read.exhausted.is_none() || read.available_lines.is_none()))
        {
            return Err("Native terminal read facts did not match the requested surface".into());
        }
        let after = self
            .transport
            .request(Method::AgentGet(schema::AgentTarget { target }))
            .map_err(|e| e.message)?;
        let ResponseResult::AgentInfo { agent: after } = after else {
            return Err("Unexpected native agent response".into());
        };
        if !identity_matches(&after) {
            return Err("Agent terminal identity changed during read; text was discarded".into());
        }
        let mut capture = json!({
            "at_ms": crate::bus::io::now_ms(),
            "source": if read_source == schema::ReadSource::Visible { "visible" } else { "recent" },
            "truncated": read.truncated,
            "revision": read.revision,
            "returned_lines": read.returned_lines,
        });
        if read_source == schema::ReadSource::Visible {
            capture["viewport"] = json!({
                "rows": read.viewport_rows,
                "columns": read.viewport_columns,
            });
        } else {
            capture["requested_lines"] = json!(read.requested_lines);
            capture["available_lines"] = json!(read.available_lines);
            capture["exhausted"] = json!(read.exhausted);
        }
        Ok(json!({
            "agent_id": id,
            "name": name,
            "status": status,
            "current_request": current_request,
            "runtime": {
                "launch_id": identity.launch_id,
                "session_id": identity.session_id,
                "pane_id": identity.pane_id,
                "terminal_id": identity.terminal_id,
            },
            "capture": capture,
            "text": read.text,
        }))
    }

    pub(super) fn approve_permission_once(
        &mut self,
        actor: crate::bus::orchestrator::ParticipantId,
        expected_agent: Option<AgentId>,
        grant: crate::bus::orchestrator::ExactPermissionGrant,
    ) -> Result<Value, String> {
        if grant.response != crate::bus::orchestrator::ApprovedPermissionResponse::AllowOnce {
            return Err("Permission response is not allowlisted".into());
        }
        let claims = decode_permission_fingerprint(&grant.fingerprint)?;
        if expected_agent.is_some_and(|agent| agent != claims.agent_id) {
            return Err("Permission fingerprint targets another agent".into());
        }
        if !self.state.orchestrator_state().authorized(
            claims.room_id,
            actor.clone(),
            crate::bus::orchestrator::Capability::ApprovePermissionOnce,
        ) {
            return Err("Participant lacks ApprovePermissionOnce capability".into());
        }
        let agent = self.state.agent(claims.agent_id).ok_or("Unknown agent")?;
        let identity = &agent.runtime_identity;
        let request = agent
            .current_request
            .ok_or("Agent has no current Request")?;
        let turn = self
            .state
            .request(request)
            .and_then(|request| request.provider_turn_id.as_deref())
            .ok_or("Current Request has no provider turn")?;
        if claims.room_id != agent.room_id
            || claims.participant_incarnation != 1
            || identity.launch_id.as_deref() != Some(claims.launch_id.as_str())
            || identity.terminal_id.as_deref() != Some(claims.terminal_id.as_str())
            || identity.session_id.as_deref() != Some(claims.session_id.as_str())
            || identity.pane_id.as_deref() != Some(claims.pane_id.as_str())
            || claims.current_request != request
            || claims.provider_turn != turn
        {
            return Err("Permission fingerprint no longer matches Worker-owned room facts".into());
        }
        if self.state.orchestrator_state().has_operation_intent(
            agent.room_id,
            "approve_permission_once",
            &grant.fingerprint,
        ) {
            return Err("Permission fingerprint was already used".into());
        }
        let room_id = agent.room_id;
        let target = claims.pane_id.clone();
        let mut intent_state = self.state.clone();
        let intent = intent_state
            .orchestrator_state_mut()
            .begin_operation(
                room_id,
                actor,
                "approve_permission_once",
                &grant.fingerprint,
            )
            .map_err(|error| format!("Permission intent rejected: {error:?}"))?;
        self.save(intent_state)?;
        let native =
            self.transport
                .request(Method::AgentApproveOnce(schema::AgentApproveOnceParams {
                    target,
                    expected_terminal_id: claims.terminal_id,
                    expected_pane_id: claims.pane_id,
                    expected_session_id: claims.session_id,
                    expected_content_revision: claims.content_revision,
                    expected_prompt_digest: claims.prompt_digest,
                    response: schema::ApprovedPermissionResponse::AllowOnce,
                }));
        let mut settled = self.state.clone();
        match native {
            Ok(ResponseResult::AgentApprovedOnce { approval }) if approval.written => {
                settled
                    .orchestrator_state_mut()
                    .reconcile_operation(
                        intent.operation_id,
                        crate::bus::orchestrator::OperationResult::Applied {
                            receipt_digest: grant.fingerprint,
                        },
                    )
                    .map_err(|error| format!("Permission settlement failed: {error:?}"))?;
                self.save(settled)?;
                Ok(json!({
                    "operation_id": intent.operation_id.0,
                    "written": true,
                    "single_use": true,
                    "audit": approval,
                }))
            }
            Ok(ResponseResult::AgentApprovedOnce { approval }) => {
                settled
                    .orchestrator_state_mut()
                    .reconcile_operation(
                        intent.operation_id,
                        crate::bus::orchestrator::OperationResult::Rejected {
                            code: approval
                                .reason
                                .clone()
                                .unwrap_or_else(|| "not_written".into()),
                        },
                    )
                    .map_err(|error| format!("Permission settlement failed: {error:?}"))?;
                self.save(settled)?;
                Err(format!(
                    "Permission was not written: {}",
                    approval.reason.unwrap_or_else(|| "prompt changed".into())
                ))
            }
            Ok(_) => {
                settled
                    .orchestrator_state_mut()
                    .mark_operation_uncertain(intent.operation_id, "unexpected native response")
                    .map_err(|error| format!("Permission uncertainty failed: {error:?}"))?;
                self.save(settled)?;
                Err("Unexpected native permission response; outcome is uncertain".into())
            }
            Err(error) => {
                settled
                    .orchestrator_state_mut()
                    .mark_operation_uncertain(intent.operation_id, &error.message)
                    .map_err(|state_error| {
                        format!("Permission uncertainty failed: {state_error:?}")
                    })?;
                self.save(settled)?;
                Err(format!(
                    "Permission outcome is uncertain: {}",
                    error.message
                ))
            }
        }
    }

    pub(super) fn permission_candidate(
        &mut self,
        id: AgentId,
    ) -> Result<PermissionCandidate, String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        let identity = agent.runtime_identity.clone();
        let room_id = agent.room_id;
        let current_request = agent
            .current_request
            .ok_or("Agent has no current Request")?;
        let provider_turn = self
            .state
            .request(current_request)
            .and_then(|request| request.provider_turn_id.clone())
            .ok_or("Current Request has no provider turn")?;
        let (Some(launch_id), Some(terminal_id), Some(session_id), Some(pane_id)) = (
            identity.launch_id,
            identity.terminal_id,
            identity.session_id,
            identity.pane_id,
        ) else {
            return Err("Agent runtime identity is incomplete".into());
        };
        let response = self
            .transport
            .request(Method::AgentPermissionObserve(schema::AgentTarget {
                target: pane_id.clone(),
            }))
            .map_err(|error| error.message)?;
        let ResponseResult::AgentPermission { observation } = response else {
            return Err("Unexpected native permission response".into());
        };
        if observation.terminal_id != terminal_id
            || observation.pane_id != pane_id
            || observation.session_id != session_id
            || !matches!(
                observation.eligibility,
                schema::PermissionEligibility::Allowlisted { .. }
            )
            || observation.allowed_responses != [schema::ApprovedPermissionResponse::AllowOnce]
        {
            return Err(
                "Permission prompt is stale, unknown, risky, or outside the allowlist".into(),
            );
        }
        let claims = PermissionFingerprintClaims {
            room_id,
            agent_id: id,
            participant_incarnation: 1,
            launch_id,
            terminal_id,
            session_id,
            pane_id,
            current_request,
            provider_turn,
            content_revision: observation.content_revision,
            prompt_digest: observation.prompt_digest.clone(),
        };
        let fingerprint = encode_permission_fingerprint(&claims)?;
        Ok(PermissionCandidate {
            claims,
            native: observation,
            fingerprint,
        })
    }
}

fn encode_permission_fingerprint(claims: &PermissionFingerprintClaims) -> Result<String, String> {
    use base64::Engine;
    let payload = serde_json::to_vec(claims).map_err(|error| error.to_string())?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload);
    let digest = format!("{:x}", Sha256::digest(&payload));
    Ok(format!("v1.{encoded}.{digest}"))
}

fn decode_permission_fingerprint(value: &str) -> Result<PermissionFingerprintClaims, String> {
    use base64::Engine;
    let mut parts = value.split('.');
    let (Some("v1"), Some(encoded), Some(expected), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("Permission fingerprint is malformed".into());
    };
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "Permission fingerprint is malformed".to_owned())?;
    let actual = format!("{:x}", Sha256::digest(&payload));
    if actual != expected {
        return Err("Permission fingerprint digest mismatch".into());
    }
    serde_json::from_slice(&payload).map_err(|_| "Permission fingerprint is malformed".into())
}

pub(super) fn permission_candidate_json(candidate: PermissionCandidate) -> Value {
    json!({
        "fingerprint": candidate.fingerprint,
        "room_id": candidate.claims.room_id,
        "agent_id": candidate.claims.agent_id,
        "participant_incarnation": candidate.claims.participant_incarnation,
        "launch_id": candidate.claims.launch_id,
        "terminal_id": candidate.claims.terminal_id,
        "session_id": candidate.claims.session_id,
        "pane_id": candidate.claims.pane_id,
        "current_request": candidate.claims.current_request,
        "provider_turn": candidate.claims.provider_turn,
        "content_revision": candidate.claims.content_revision,
        "prompt_digest": candidate.claims.prompt_digest,
        "prompt_text": candidate.native.prompt_text,
        "eligibility": candidate.native.eligibility,
        "allowed_responses": candidate.native.allowed_responses,
        "observed_at_ms": crate::bus::io::now_ms(),
    })
}

fn required<'a>(p: &'a Value, field: &str) -> Result<&'a str, String> {
    p.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("Missing or invalid {field}"))
}
fn optional_text<'a>(p: &'a Value, field: &str) -> Result<Option<&'a str>, String> {
    p.get(field)
        .map(|v| v.as_str().ok_or_else(|| format!("Invalid {field}")))
        .transpose()
}
fn optional_bool(p: &Value, field: &str) -> Result<bool, String> {
    p.get(field).map_or(Ok(false), |v| {
        v.as_bool().ok_or_else(|| format!("Invalid {field}"))
    })
}
fn optional_u32(p: &Value, field: &str) -> Result<Option<u32>, String> {
    p.get(field)
        .map(|value| {
            value
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|&n| n > 0)
                .ok_or_else(|| format!("Invalid {field}"))
        })
        .transpose()
}
fn unique<T>(mut items: impl Iterator<Item = T>) -> Result<T, String> {
    let first = items.next().ok_or("No matching room or agent")?;
    if items.next().is_some() {
        Err("Ambiguous name; use a numeric ID".into())
    } else {
        Ok(first)
    }
}

#[cfg(test)]
#[path = "runtime_control_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "runtime_focus_tests.rs"]
mod focus_tests;
