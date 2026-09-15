//! Dev commands execute on the existing single-writer coordinator.
use super::*;
use crate::bus::control::{Request as ControlRequest, Response};
use serde_json::{json, Value};

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
                .collect::<Result<BTreeSet<_>, _>>()?
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
        Ok(json!({"message_id":message,"request_ids":ids,"stage":"queued"}))
    }

    fn dev_message(&self, message: PromptId) -> Result<Value, String> {
        let requests = self
            .state
            .requests()
            .filter(|r| r.prompt.id == message)
            .collect::<Vec<_>>();
        if requests.is_empty() {
            return Err("Unknown message ID".into());
        }
        Ok(
            json!({"message_id":message,"complete":requests.iter().all(|r|matches!(r.phase,RequestPhase::Completed|RequestPhase::Abandoned)),"requests":requests.iter().map(|r| {
            let agent = self.state.agent(r.agent_id);
            let stage = match r.phase { RequestPhase::Queued=>"queued",RequestPhase::Submitting=>"submitting",RequestPhase::Active if r.trusted_start_bound=>"delivered",RequestPhase::Active=>"awaiting_start",RequestPhase::Completed=>"replied",RequestPhase::Abandoned=>"abandoned" };
            json!({"request_id":r.id,"agent_id":r.agent_id,"agent_name":agent.map(|a|&a.name),"stage":stage,"reason":if r.phase==RequestPhase::Queued {agent.and_then(crate::bus::diagnostics::wait_reason)}else{None},"status":agent.map(|a|a.status),"uncertain_outcome":r.uncertain_outcome,"session_id":r.provider_session_id,"turn_id":r.provider_turn_id,"start_bound":r.trusted_start_bound,"reply":if r.phase==RequestPhase::Completed {r.pending_final.as_ref()}else{None}})
        }).collect::<Vec<_>>()}),
        )
    }

    fn dev_read(
        &mut self,
        id: AgentId,
        source: Option<&str>,
        lines: Option<u32>,
    ) -> Result<Value, String> {
        let read_source = match source {
            None | Some("recent") => schema::ReadSource::Recent,
            Some("visible") => schema::ReadSource::Visible,
            Some(_) => return Err("Source must be visible or recent".into()),
        };
        if read_source == schema::ReadSource::Visible && lines.is_some() {
            return Err("Visible reads return the complete viewport; omit lines".into());
        }
        let inspect = source.is_some() || lines.is_some();
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        let target = agent
            .runtime_identity
            .pane_id
            .clone()
            .ok_or("Agent has no terminal")?;
        let identity = agent.runtime_identity.clone();
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
        if Some(&info.terminal_id) != identity.terminal_id.as_ref()
            || Some(&info.pane_id) != identity.pane_id.as_ref()
            || info.name.as_deref() != Some(&expected_name)
            || identity
                .session_id
                .as_ref()
                .is_some_and(|s| info.agent_session.as_ref().is_none_or(|v| &v.value != s))
        {
            return Err("Agent terminal identity changed; inspect the owned session".into());
        }
        const NATIVE_RECENT_MAX_LINES: u32 = 1000;
        let native_lines = match read_source {
            schema::ReadSource::Visible => None,
            schema::ReadSource::Recent if !inspect => Some(400),
            schema::ReadSource::Recent => lines.map(|n| n.min(NATIVE_RECENT_MAX_LINES)),
            _ => return Err("Source must be visible or recent".into()),
        };
        let viewport = if read_source == schema::ReadSource::Visible {
            self.transport
                .request(Method::PaneGet(schema::PaneTarget {
                    pane_id: target.clone(),
                }))
                .ok()
                .and_then(|response| match response {
                    ResponseResult::PaneInfo { pane } => pane.scroll.map(|scroll| {
                        json!({
                            "rows": scroll.viewport_rows,
                            "offset_from_bottom": scroll.offset_from_bottom,
                            "max_offset_from_bottom": scroll.max_offset_from_bottom,
                        })
                    }),
                    _ => None,
                })
        } else {
            None
        };
        let output = self
            .transport
            .request(Method::AgentRead(schema::AgentReadParams {
                target,
                source: read_source,
                lines: native_lines,
                format: schema::ReadFormat::Text,
                strip_ansi: true,
            }))
            .map_err(|e| e.message)?;
        if !inspect {
            return Ok(json!({"agent_id":id,"runtime":info,"output":output}));
        }
        let ResponseResult::PaneRead { read } = output else {
            return Err("Unexpected native read response".into());
        };
        let mut capture = json!({
            "at_ms": crate::bus::io::now_ms(),
            "source": if read_source == schema::ReadSource::Visible { "visible" } else { "recent" },
            "truncated": read.truncated,
            "more": read.truncated,
            "revision": read.revision,
        });
        if read_source == schema::ReadSource::Visible {
            if let Some(viewport) = viewport {
                capture["viewport"] = viewport;
            }
        } else {
            let honored = native_lines;
            let capped = lines.is_some_and(|n| n > NATIVE_RECENT_MAX_LINES);
            capture["requested_lines"] = json!(lines);
            capture["lines"] = json!(honored);
            capture["native_max_lines"] = json!(NATIVE_RECENT_MAX_LINES);
            capture["resumable"] =
                json!(read.truncated && honored.is_none_or(|n| n < NATIVE_RECENT_MAX_LINES));
            if capped {
                capture["limit"] = json!({
                    "applied": true,
                    "source": "native AgentRead/pane.read caps lines at 1000",
                });
            }
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
