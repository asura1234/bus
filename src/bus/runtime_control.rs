//! Dev commands execute on the existing single-writer coordinator.
use super::*;
use crate::bus::control::{Request as ControlRequest, Response};
use crate::bus::orchestrator::OrchestratorSpec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// What a dialog fingerprint vouches for: this agent's launch identity and
/// the exact dialog it showed. A fingerprint answers at most one dialog.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct DialogFingerprintClaims {
    room_id: RoomId,
    agent_id: AgentId,
    launch_id: String,
    terminal_id: String,
    pane_id: String,
    /// `None` while the agent launches, before its session is bound.
    session_id: Option<String>,
    content_revision: u64,
    dialog_digest: String,
    /// The question and labels without the selection, to tell a moved
    /// selection from a different dialog.
    dialog_shape: String,
    options: u32,
    /// Makes each observation's fingerprint distinct, so spending one never
    /// blocks answering the same dialog after observing it again.
    observed_at_ns: u128,
}

/// How long `agent choose` watches the screen for the dialog to react.
const DIALOG_SETTLE_POLLS: u32 = 20;
const DIALOG_SETTLE_INTERVAL: Duration = Duration::from_millis(100);

/// Receipts younger than this are never evicted, so a retry within it replays.
pub(super) const DEV_RECEIPT_RETENTION: Duration = Duration::from_secs(10 * 60);
/// At most this many receipts are kept inside the retention window.
const DEV_RECEIPT_LIMIT: usize = 1024;

/// A committed mutation's response, replayed for a retry with the same ID.
pub(super) struct DevReceipt {
    request: ControlRequest,
    response: Response,
    at: std::time::Instant,
    bytes: usize,
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
        if let Some(receipt) = self.dev_receipts.get(&request.id) {
            return if receipt.request.method == request.method
                && receipt.request.params == request.params
            {
                receipt.response.clone()
            } else {
                Response::failure(
                    &request.id,
                    "id_conflict",
                    "Request ID already used with different parameters",
                )
            };
        }
        let (fields, mutation): (&[&str], bool) = match request.method.as_str() {
            "state" | "diagnostics" | "sounds" | "settings" => (&[], false),
            "room.create" => (&["name"], true),
            "room.rename" => (&["room", "name"], true),
            "room.notes" => (&["room", "text"], true),
            "room.delete" => (&["room", "confirm"], true),
            "room.focus" => (&["room"], true),
            "room.seen" => (&["room"], true),
            "room.sound" => (&["room", "on", "sound"], true),
            "agent.rename" => (&["agent", "name"], true),
            "agent.details" => (&["agent", "on"], true),
            "settings.color_blind" => (&["on"], true),
            "settings.room_sound" => (&["on", "sound"], true),
            "bus.quit" => (&[], true),
            "agent.add" => (
                &[
                    "room",
                    "name",
                    "provider",
                    "cwd",
                    "extra_args",
                    "consent_project_hooks",
                    "orchestrates",
                    "system_prompt",
                ],
                true,
            ),
            "agent.orchestrate" => (&["agent", "room"], true),
            "agent.delete" | "agent.setup-confirm" => (&["agent", "confirm"], true),
            "agent.read" => (&["agent", "source", "lines"], false),
            "agent.dialog.observe" => (&["agent"], false),
            "agent.dialog.choose" => (&["agent", "option", "fingerprint"], true),
            "agent.dialog.answer" => (&["agent", "text", "skip", "fingerprint"], true),
            "agent.focus" => (&["agent"], true),
            "agent.clear" => (&["agent"], true),
            "message.send" => (&["room", "to", "text", "files", "as", "queue"], true),
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
        // A receipt lets a retry replay its response instead of launching or
        // sending twice. Retries come within seconds, so receipts older than
        // the retention window make room; younger ones are never evicted.
        let reserve = serde_json::to_vec(request)
            .map_or(usize::MAX, |v| v.len())
            .saturating_add(4096);
        if mutation {
            self.evict_expired_dev_receipts();
        }
        if mutation
            && (self.dev_receipts.len() >= DEV_RECEIPT_LIMIT
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
            self.dev_receipt_order.push_back(request.id.clone());
            self.dev_receipts.insert(
                request.id.clone(),
                DevReceipt {
                    request: request.clone(),
                    response: response.clone(),
                    at: std::time::Instant::now(),
                    bytes: reserve,
                },
            );
        }
        response
    }

    fn evict_expired_dev_receipts(&mut self) {
        while let Some(id) = self.dev_receipt_order.front() {
            let Some(receipt) = self.dev_receipts.get(id) else {
                self.dev_receipt_order.pop_front();
                continue;
            };
            if receipt.at.elapsed() < self.dev_receipt_retention {
                break;
            }
            self.dev_receipt_bytes = self.dev_receipt_bytes.saturating_sub(receipt.bytes);
            self.dev_receipts.remove(id);
            self.dev_receipt_order.pop_front();
        }
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
                json!({"revision":self.revision,"master_room":self.state.master_room().map(|r|r.id),"visible_room":self.state.visible_room(),"rooms":self.state.rooms().map(|r|json!({"id":r.id,"name":r.name,"kind":r.kind,"notes":r.notes,"unread_count":r.unread_count,"status":self.state.room_status(r.id),"sound":r.sound_enabled(),"sound_name":r.sound_name.as_deref().unwrap_or(crate::sound::DEFAULT_SOUND_NAME),"deletion_pending":r.deletion_pending,"orchestrator":self.state.orchestrator_of(r.id).map(|a|a.id)})).collect::<Vec<_>>(),"agents":self.state.agents().collect::<Vec<_>>(),"usage":self.usage.state_json(),"settings":self.settings_json(),"build":build_json()}),
            ),
            "diagnostics" => Ok(
                json!({"version":env!("CARGO_PKG_VERSION"),"dev":true,"storage_failed":self.storage_failed,"coordinator_error":self.error,"data_dir":self.data_dir,"logs":self.data_dir.join("herdr-config/sessions/bus"),"callback_logs":self.data_dir.join("callbacks"),"agents":self.state.agents().map(|a|json!({"agent_id":a.id,"name":a.name,"status":a.status,"reason":crate::bus::diagnostics::wait_reason(a),"detail":a.actionable_error,"identity":a.runtime_identity,"current_request":a.current_request})).collect::<Vec<_>>()}),
            ),
            "room.create" => self.dev_command(BusCommand::CreateRoom(required(p, "name")?.into())),
            "room.rename" => self.dev_command(BusCommand::RenameRoom(
                self.dev_room(required(p, "room")?)?,
                required(p, "name")?.into(),
            )),
            "room.notes" => self.dev_command(BusCommand::SetNotes(
                self.dev_room(required(p, "room")?)?,
                optional_text(p, "text")?.ok_or("Missing text")?.into(),
            )),
            "room.seen" => self.dev_command(BusCommand::MarkRoomSeen(
                self.dev_room(required(p, "room")?)?,
            )),
            "sounds" => Ok(json!({
                "sounds": std::iter::once(json!({"name": crate::sound::DEFAULT_SOUND_NAME, "path": null}))
                    .chain(self.system_sounds().into_iter().map(|sound| json!({"name": sound.name, "path": sound.path})))
                    .collect::<Vec<_>>(),
            })),
            "room.sound" => {
                let room = self.dev_room(required(p, "room")?)?;
                let on = p
                    .get("on")
                    .and_then(Value::as_bool)
                    .ok_or("Sound must be on or off")?;
                // Resolve the name before changing anything, so an unknown
                // sound leaves the room as it was.
                let name = self.sound_choice(p)?;
                if let Some(name) = name {
                    self.dev_command(BusCommand::SetRoomSoundName(room, name))?;
                }
                self.dev_command(BusCommand::SetRoomSound(room, on))
            }
            "agent.details" => self.dev_command(BusCommand::SetDetails(
                self.dev_agent(required(p, "agent")?, None)?,
                p.get("on")
                    .and_then(Value::as_bool)
                    .ok_or("Details must be on or off")?,
            )),
            "settings.color_blind" => {
                let on = p
                    .get("on")
                    .and_then(Value::as_bool)
                    .ok_or("Color-blind mode must be on or off")?;
                let events = events.ok_or("Bus UI event channel unavailable")?;
                let path = self
                    .settings_path
                    .as_ref()
                    .ok_or("Bus settings location unavailable")?;
                // Change only this field of the saved file; other writers keep theirs.
                let settings =
                    crate::bus::settings::update(path, |settings| settings.color_blind_mode = on)?;
                events
                    .send(BusEvent::SettingsChanged(settings))
                    .map_err(|_| "Bus UI event channel disconnected")?;
                Ok(json!({"updated":true,"color_blind_mode":on}))
            }
            "settings" => Ok(json!({"settings": self.settings_json()})),
            "settings.room_sound" => {
                let on = p
                    .get("on")
                    .and_then(Value::as_bool)
                    .ok_or("Sound must be on or off")?;
                let name = self.sound_choice(p)?;
                let events = events.ok_or("Bus UI event channel unavailable")?;
                let saved = self.update_new_room_sound(
                    |pref| {
                        pref.enabled = on;
                        if let Some(name) = name {
                            pref.name = name;
                        }
                    },
                    events,
                )?;
                Ok(json!({"updated":true,"room_sound":saved.room_sound}))
            }
            "bus.quit" => {
                // Same as Ctrl+Q: the UI saves drafts, then shuts the coordinator down.
                events
                    .ok_or("Bus UI event channel unavailable")?
                    .send(BusEvent::DevQuitRequested)
                    .map_err(|_| "Bus UI event channel disconnected")?;
                Ok(json!({"stage":"queued"}))
            }
            "agent.rename" => self.dev_command(BusCommand::RenameAgent(
                self.dev_agent(required(p, "agent")?, None)?,
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
                let room = self.dev_room(required(p, "room")?)?;
                let master = self.state.master_room().is_some_and(|m| m.id == room);
                let orchestrates = optional_text(p, "orchestrates")?;
                let system_prompt = optional_text(p, "system_prompt")?;
                if !master && system_prompt.is_some() {
                    return Err("--system-prompt applies only to MASTER orchestrators".into());
                }
                let input = AddAgent {
                    room,
                    name: required(p, "name")?.into(),
                    provider,
                    cwd: required(p, "cwd")?.into(),
                    extra_args: optional_text(p, "extra_args")?.unwrap_or_default().into(),
                    consent_project_hooks: optional_bool(p, "consent_project_hooks")?,
                };
                // Every MASTER agent is an orchestrator; outside MASTER,
                // --orchestrates still reaches the model's MASTER-only check.
                self.dev_command(if master || orchestrates.is_some() {
                    let spec = OrchestratorSpec {
                        room: orchestrates.map(|r| self.dev_room(r)).transpose()?,
                        system_prompt: system_prompt.map(Into::into),
                    };
                    BusCommand::AddOrchestrator(input, spec)
                } else {
                    BusCommand::AddAgent(input)
                })
            }
            "agent.orchestrate" => {
                let agent = self.dev_agent(required(p, "agent")?, None)?;
                let room = match p.get("room") {
                    Some(Value::Null) => None,
                    Some(Value::String(room)) => Some(self.dev_room(room)?),
                    _ => return Err("Room must be a room selector or null".into()),
                };
                self.dev_command(BusCommand::SetOrchestrates(agent, room))
            }
            "agent.read" => self.dev_read(
                self.dev_agent(required(p, "agent")?, None)?,
                optional_text(p, "source")?,
                optional_u32(p, "lines")?,
            ),
            "agent.dialog.observe" => {
                let agent = self.dev_agent(required(p, "agent")?, None)?;
                self.observe_dialog(agent)
            }
            "agent.dialog.choose" => {
                let agent = self.dev_agent(required(p, "agent")?, None)?;
                let option = required(p, "option")?
                    .parse::<u32>()
                    .map_err(|_| "Option must be a number")?;
                self.choose_dialog_option(agent, option, required(p, "fingerprint")?)
            }
            "agent.dialog.answer" => {
                let agent = self.dev_agent(required(p, "agent")?, None)?;
                let text = if p["text"].is_null() {
                    None
                } else {
                    optional_text(p, "text")?
                };
                let skip = optional_bool(p, "skip")?;
                self.answer_dialog(agent, text, skip, required(p, "fingerprint")?)
            }
            "agent.clear" => {
                let agent = self.dev_agent(required(p, "agent")?, None)?;
                self.dev_clear(agent)
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
                    .filter(|r| r.room_id == room && !r.delivery_only())
                    .map(|r| (r.prompt.id, &r.prompt))
                    .collect::<BTreeMap<_, _>>();
                Ok(
                    json!({"room_id":room,"messages":messages.iter().map(|(id,prompt)|json!({"prompt":prompt,"delivery":self.dev_message(*id).ok()})).collect::<Vec<_>>()}),
                )
            }
            _ => Err("Unknown method".into()),
        }
    }

    /// Starts a fresh provider context in an idle agent's terminal. The reset
    /// command is typed directly, not sent as a Bus message, and the agent
    /// rebinds to the new provider session its next callback reports.
    fn dev_clear(&mut self, id: AgentId) -> Result<Value, String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        if agent.deletion_pending
            || agent.session_binding_invalidated
            || !agent.hook_setup_confirmed
            || agent.status != RuntimeStatus::Idle
        {
            return Err("Only an idle, ready agent can be cleared".into());
        }
        if agent.current_request.is_some() || self.state.next_queued_request(id).is_some() {
            return Err(
                "The agent still has messages to answer; clear it once they are done".into(),
            );
        }
        let identity = &agent.runtime_identity;
        let (Some(terminal), Some(pane)) = (&identity.terminal_id, &identity.pane_id) else {
            return Err("The agent has no terminal yet".into());
        };
        let text = clear_command(agent.provider).to_string();
        let method = match &identity.session_id {
            Some(session) => Method::AgentPromptIfIdle(schema::AgentPromptIfIdleParams {
                target: pane.clone(),
                text: text.clone(),
                expected_terminal_id: terminal.clone(),
                expected_pane_id: pane.clone(),
                expected_agent: launch::provider_kind(agent.provider).into(),
                expected_session_id: session.clone(),
                steer: false,
            }),
            // Codex starts its provider session with the first turn.
            None if agent.provider == Provider::Codex => {
                Method::AgentPromptIfUnbound(schema::AgentPromptIfUnboundParams {
                    target: pane.clone(),
                    text: text.clone(),
                    expected_terminal_id: terminal.clone(),
                    expected_pane_id: pane.clone(),
                    expected_managed_name: format!("bus-r{}-a{}", agent.room_id.0, agent.id.0),
                })
            }
            None => return Err("The agent has no provider session yet".into()),
        };
        match self.transport.request(method) {
            Ok(ResponseResult::AgentPrompted { .. }) => {}
            Ok(other) => return Err(format!("Unexpected response to {text}: {other:?}")),
            Err(error) => return Err(error.message),
        }
        let mut state = self.state.clone();
        state.begin_session_reset(id).map_err(|e| e.to_string())?;
        self.save(state)?;
        Ok(json!({"agent_id": id, "sent": text, "stage": "cleared"}))
    }

    fn settings_json(&self) -> Value {
        match self
            .settings_path
            .as_deref()
            .map(crate::bus::settings::load)
        {
            Some(Ok(settings)) => json!(settings),
            Some(Err(error)) => json!({"error": error}),
            None => Value::Null,
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
                BusEvent::TerminalsLeftOpen(terminals) => {
                    return Ok(json!({"updated":true,"terminals_left_open":terminals}))
                }
                _ => {}
            }
        }
        Ok(json!({"updated":true}))
    }

    /// The optional `sound` parameter: None when absent, Some(None) for Bus's
    /// own ding, or an installed system sound by its listed name.
    fn sound_choice(&self, p: &Value) -> Result<Option<Option<String>>, String> {
        Ok(match optional_text(p, "sound")? {
            None => None,
            Some(name) if name.eq_ignore_ascii_case(crate::sound::DEFAULT_SOUND_NAME) => Some(None),
            Some(name) => Some(Some(
                crate::sound::find_sound(&self.system_sounds(), name)
                    .map(|sound| sound.name.clone())
                    .ok_or_else(|| format!("Unknown sound {name:?}; `bus sounds` lists them"))?,
            )),
        })
    }

    fn system_sounds(&self) -> Vec<crate::sound::SystemSound> {
        match &self.sound_dirs {
            Some(dirs) => crate::sound::list_sounds(dirs),
            None => crate::sound::system_sounds(),
        }
    }

    fn dev_room(&self, selector: &str) -> Result<RoomId, String> {
        if selector.eq_ignore_ascii_case(crate::bus::model::MASTER_ROOM_NAME) {
            if let Some(master) = self.state.master_room() {
                return Ok(master.id);
            }
        }
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
            .any(|s| s.eq_ignore_ascii_case(crate::bus::model::HUMAN_RECIPIENT));
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
                crate::bus::io::now_ms(),
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
            crate::bus::diagnostics::request(&self.state, *id, "bus.message.queued", "persisted");
        }
        Ok(json!({"message_id":message,"request_ids":ids,"stage":"queued"}))
    }

    /// `send --to human`: an agent's message to the Human in MASTER. It is
    /// delivered to no agent, so it has no requests and nothing to wait for.
    fn dev_send_to_human(
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
                crate::bus::io::now_ms(),
            )
            .map_err(|e| e.to_string())?;
        self.save(state)?;
        Ok(json!({"message_id":message,"request_ids":[],"stage":"posted"}))
    }

    pub(super) fn dev_message(&self, message: PromptId) -> Result<Value, String> {
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
            let stage = match r.phase { RequestPhase::Queued=>"queued",RequestPhase::Submitting=>"submitting",RequestPhase::Active if r.group.is_some()=>"joined",RequestPhase::Active if r.trusted_start_bound=>"delivered",RequestPhase::Active=>"awaiting_start",RequestPhase::Completed=>"replied",RequestPhase::Abandoned=>"abandoned" };
            json!({"request_id":r.id,"agent_id":r.agent_id,"agent_name":agent.map(|a|&a.name),"stage":stage,"reason":if r.phase==RequestPhase::Queued {agent.and_then(crate::bus::diagnostics::wait_reason)}else{None},"status":agent.map(|a|a.status),"uncertain_outcome":r.uncertain_outcome,"session_id":r.provider_session_id,"turn_id":r.provider_turn_id,"start_bound":r.trusted_start_bound,"dialog":self.waiting_on_dialog(r),"group":r.group,"queue":r.queue_only,"reply":if r.phase==RequestPhase::Completed {r.pending_final.as_ref()}else{None}})
        }).collect::<Vec<_>>()}),
        )
    }

    /// An unsettled request whose agent shows a dialog or a blocked screen.
    fn waiting_on_dialog(&self, request: &Request) -> bool {
        !matches!(
            request.phase,
            RequestPhase::Completed | RequestPhase::Abandoned
        ) && self
            .state
            .agent(request.agent_id)
            .is_some_and(|agent| agent.dialog || agent.status == RuntimeStatus::Blocked)
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
        if identity.launch_id.is_none() || identity.terminal_id.is_none() {
            return Err("Agent runtime identity is incomplete".into());
        }
        // A launching agent has a pane before its provider session starts, and
        // its terminal may be waiting on a prompt such as Claude's trust dialog.
        // Reading is still allowed then; only the session check is skipped.
        let session_verified = identity.session_id.is_some();
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
                && (!session_verified
                    || info
                        .agent_session
                        .as_ref()
                        .map(|value| value.value.as_str())
                        == identity.session_id.as_deref())
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
                "session_verified": session_verified,
                "pane_id": identity.pane_id,
                "terminal_id": identity.terminal_id,
            },
            "capture": capture,
            "text": read.text,
        }))
    }

    /// The agent's launch identity: launch, terminal and pane are required;
    /// the session is `None` until the provider binds it.
    fn dialog_identity(
        &self,
        id: AgentId,
    ) -> Result<(RoomId, String, String, String, Option<String>), String> {
        let agent = self.state.agent(id).ok_or("Unknown agent")?;
        let identity = &agent.runtime_identity;
        let (Some(launch_id), Some(terminal_id), Some(pane_id)) = (
            identity.launch_id.clone(),
            identity.terminal_id.clone(),
            identity.pane_id.clone(),
        ) else {
            return Err("Agent has no terminal yet".into());
        };
        Ok((
            agent.room_id,
            launch_id,
            terminal_id,
            pane_id,
            identity.session_id.clone(),
        ))
    }

    fn native_dialog(
        &mut self,
        pane_id: &str,
        terminal_id: &str,
        session_id: Option<&str>,
    ) -> Result<schema::AgentDialogObservation, String> {
        let response = self
            .transport
            .request(Method::AgentDialogObserve(schema::AgentTarget {
                target: pane_id.into(),
            }))
            .map_err(|error| error.message)?;
        let ResponseResult::AgentDialog { observation } = response else {
            return Err("Unexpected native dialog response".into());
        };
        if observation.terminal_id != terminal_id
            || observation.pane_id != pane_id
            || session_id.is_some_and(|session| observation.session_id.as_deref() != Some(session))
        {
            return Err("Agent terminal or session changed; observe the dialog again".into());
        }
        Ok(observation)
    }

    pub(super) fn observe_dialog(&mut self, id: AgentId) -> Result<Value, String> {
        let (room_id, launch_id, terminal_id, pane_id, session_id) = self.dialog_identity(id)?;
        let observation = self.native_dialog(&pane_id, &terminal_id, session_id.as_deref())?;
        let fingerprint = match &observation.dialog {
            Some(dialog) => Some(encode_dialog_fingerprint(&DialogFingerprintClaims {
                room_id,
                agent_id: id,
                launch_id,
                terminal_id,
                pane_id,
                session_id,
                content_revision: observation.content_revision,
                dialog_digest: dialog.digest.clone(),
                dialog_shape: dialog.id.clone(),
                options: dialog.options.len() as u32,
                observed_at_ns: crate::bus::io::now_ns(),
            })?),
            None => None,
        };
        Ok(json!({
            "agent_id": id,
            "dialog": observation.dialog.as_ref().map(dialog_json),
            "fingerprint": fingerprint,
            "content_revision": observation.content_revision,
            "observed_at_ms": crate::bus::io::now_ms(),
        }))
    }

    /// Answers the observed dialog once: the fingerprint is spent before any
    /// key is written, so a lost response can never answer a second dialog.
    pub(super) fn choose_dialog_option(
        &mut self,
        id: AgentId,
        option: u32,
        fingerprint: &str,
    ) -> Result<Value, String> {
        self.send_dialog_answer(id, Some(option), None, false, fingerprint)
    }

    pub(super) fn answer_dialog(
        &mut self,
        id: AgentId,
        text: Option<&str>,
        skip: bool,
        fingerprint: &str,
    ) -> Result<Value, String> {
        schema::AgentDialogAnswerParams::validate_answer(text, skip)?;
        self.send_dialog_answer(id, None, text, skip, fingerprint)
    }

    fn send_dialog_answer(
        &mut self,
        id: AgentId,
        option: Option<u32>,
        text: Option<&str>,
        skip: bool,
        fingerprint: &str,
    ) -> Result<Value, String> {
        let claims = decode_dialog_fingerprint(fingerprint)?;
        if claims.agent_id != id {
            return Err("Dialog fingerprint belongs to another agent".into());
        }
        let (room_id, launch_id, terminal_id, pane_id, session_id) = self.dialog_identity(id)?;
        if claims.room_id != room_id
            || claims.launch_id != launch_id
            || claims.terminal_id != terminal_id
            || claims.pane_id != pane_id
            || claims.session_id != session_id
        {
            return Err("Agent launch or session changed; observe the dialog again".into());
        }
        if option.is_some_and(|option| option == 0 || option > claims.options) {
            return Err(format!("Option must be between 1 and {}", claims.options));
        }
        if option.is_none() && claims.options != 0 {
            return Err("This is a choice dialog; use agent choose".into());
        }
        if self.state.dialog_fingerprint_consumed(fingerprint) {
            return Err("Dialog fingerprint was already used; observe the dialog again".into());
        }
        let mut consumed = self.state.clone();
        consumed.consume_dialog_fingerprint(fingerprint.to_owned());
        self.save(consumed)?;
        let method = match option {
            Some(option) => Method::AgentDialogChoose(schema::AgentDialogChooseParams {
                target: pane_id.clone(),
                expected_terminal_id: terminal_id.clone(),
                expected_pane_id: pane_id.clone(),
                expected_session_id: session_id.clone(),
                expected_dialog_digest: claims.dialog_digest.clone(),
                option,
            }),
            None => Method::AgentDialogAnswer(schema::AgentDialogAnswerParams {
                target: pane_id.clone(),
                expected_terminal_id: terminal_id.clone(),
                expected_pane_id: pane_id.clone(),
                expected_session_id: session_id.clone(),
                expected_dialog_digest: claims.dialog_digest.clone(),
                text: text.map(str::to_owned),
                skip,
            }),
        };
        let native = self.transport.request(method);
        let keys = match native {
            Ok(ResponseResult::AgentDialogChosen { choice }) if choice.written => choice.keys,
            Ok(ResponseResult::AgentDialogChosen { choice }) => {
                return Err(format!(
                    "No keys were sent ({}); observe the dialog again",
                    choice.reason.as_deref().unwrap_or("dialog changed")
                ))
            }
            Ok(_) => return Err("Unexpected native dialog response; outcome is uncertain".into()),
            Err(error) => {
                return Err(format!(
                    "Dialog answer outcome is uncertain: {}",
                    error.message
                ))
            }
        };
        // Its closing is expected now, so no "closed on its own" follow-up.
        if let Some(option) = option {
            let mut answered = self.state.clone();
            answered
                .mark_dialog_answered(id, option)
                .map_err(|error| error.to_string())?;
            self.save(answered)?;
        } else {
            let mut answered = self.state.clone();
            let notice = answered
                .agent(id)
                .and_then(|agent| agent.dialog_notice.clone());
            // A text answer has no numbered choice, including if it replaces
            // an earlier choice before the notice poll catches up.
            answered
                .set_dialog_notice(id, notice)
                .map_err(|error| error.to_string())?;
            self.save(answered)?;
        }
        // Moves are confirmed after a short delay, so watch until the dialog
        // closes or another one replaces it.
        let mut outcome = "unchanged";
        let mut after = None;
        for poll in 0..DIALOG_SETTLE_POLLS {
            if poll > 0 {
                std::thread::sleep(DIALOG_SETTLE_INTERVAL);
            }
            let Ok(observation) = self.native_dialog(&pane_id, &terminal_id, session_id.as_deref())
            else {
                outcome = "unknown";
                break;
            };
            outcome = match &observation.dialog {
                None => "closed",
                Some(dialog) if dialog.digest == claims.dialog_digest => "unchanged",
                Some(dialog) if dialog.id == claims.dialog_shape => {
                    if option.is_some() {
                        "selection_moved"
                    } else {
                        "input_changed"
                    }
                }
                Some(_) => "replaced",
            };
            after = observation.dialog;
            if matches!(outcome, "closed" | "replaced") {
                break;
            }
        }
        let mut result = json!({
            "agent_id": id,
            "keys": keys,
            "outcome": outcome,
            "dialog": after.as_ref().map(dialog_json),
        });
        if let Some(option) = option {
            result["option"] = json!(option);
        } else {
            result["skipped"] = json!(skip);
        }
        Ok(result)
    }
}

fn dialog_json(dialog: &schema::AgentDialog) -> Value {
    json!({
        "kind": dialog.kind,
        "text": dialog.text,
        "options": dialog.options,
        "hint": dialog.hint,
    })
}

fn encode_dialog_fingerprint(claims: &DialogFingerprintClaims) -> Result<String, String> {
    use base64::Engine;
    let payload = serde_json::to_vec(claims).map_err(|error| error.to_string())?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload);
    let digest = format!("{:x}", Sha256::digest(&payload));
    Ok(format!("d1.{encoded}.{digest}"))
}

fn decode_dialog_fingerprint(value: &str) -> Result<DialogFingerprintClaims, String> {
    use base64::Engine;
    let malformed = || "Dialog fingerprint is malformed; observe the dialog again".to_owned();
    let mut parts = value.split('.');
    let (Some("d1"), Some(encoded), Some(expected), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(malformed());
    };
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| malformed())?;
    if format!("{:x}", Sha256::digest(&payload)) != expected {
        return Err(malformed());
    }
    serde_json::from_slice(&payload).map_err(|_| malformed())
}

/// The validated, deduplicated `files` of a send.
fn attachment_files(p: &Value) -> Result<Vec<PathBuf>, String> {
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
    Ok(files)
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

/// How the running Bus was built: `debug` for a development build (`./run dev`,
/// `cargo build`), `release` for an optimized build, plus the binary's path.
fn build_json() -> Value {
    json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "binary": std::env::current_exe().ok(),
    })
}

/// The provider command that starts a fresh context in the same terminal.
/// Codex's `/new` asks where the new conversation runs; `/clear` does not.
fn clear_command(provider: Provider) -> &'static str {
    match provider {
        Provider::ClaudeCode | Provider::Codex => "/clear",
        Provider::Cursor => "/new-chat",
    }
}
