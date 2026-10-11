//! Inspect development commands on the single coordinator.
use super::{
    mpsc, optional_text, optional_u32, required, schema, AgentId, BusEvent, Method, ResponseResult,
    Worker,
};
use serde_json::{json, Value};

impl Worker {
    pub(super) fn execute_inspect(
        &mut self,
        method: &str,
        p: &Value,
        events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<Value, String> {
        match method {
            "state" => Ok(
                json!({"revision":self.revision,"master_room":self.state.master_room().map(|r|r.id),"visible_room":self.state.visible_room(),"rooms":self.state.rooms().map(|r|json!({"id":r.id,"name":r.name,"kind":r.kind,"notes":r.notes,"unread_count":r.unread_count,"status":self.state.room_status(r.id),"sound":r.sound_enabled(),"sound_name":r.sound_name.as_deref().unwrap_or(crate::platform::sound::DEFAULT_SOUND_NAME),"deletion_pending":r.deletion_pending,"orchestrator":self.state.orchestrator_of(r.id).map(|a|a.id)})).collect::<Vec<_>>(),"agents":self.state.agents().map(agent_json).collect::<Vec<_>>(),"usage":self.usage.state_json(),"settings":self.settings_json(),"build":build_json()}),
            ),
            "diagnostics" => Ok(
                json!({"version":env!("CARGO_PKG_VERSION"),"dev":true,"storage_failed":self.storage_pause.is_some(),"storage_repair_required":matches!(self.storage_pause.as_ref(),Some(super::super::StoragePause::NeedsRepair)),"coordinator_error":self.error,"data_dir":self.data_dir,"logs":self.data_dir.join("herdr-config/sessions/bus"),"callback_logs":self.data_dir.join("callbacks"),"agents":self.state.agents().map(|a|json!({"agent_id":a.id,"name":a.name,"status":a.status,"reason":crate::messaging::diagnostics::wait_reason(a),"detail":a.actionable_error,"identity":a.runtime_identity,"current_request":a.current_request})).collect::<Vec<_>>()}),
            ),
            "sounds" => Ok(json!({
                "sounds": std::iter::once(json!({"name": crate::platform::sound::DEFAULT_SOUND_NAME, "path": null}))
                    .chain(self.system_sounds().into_iter().map(|sound| json!({"name": sound.name, "path": sound.path})))
                    .collect::<Vec<_>>(),
            })),
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
                let settings = crate::messaging::prefs::settings::update(path, |settings| {
                    settings.color_blind_mode = on
                })?;
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
                let saved = self.set_all_rooms_sound(Some(on), name, events)?;
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
            "agent.read" => self.dev_read(
                self.dev_agent(required(p, "agent")?, None)?,
                optional_text(p, "source")?,
                optional_u32(p, "lines")?,
            ),
            _ => Err("Unknown method".into()),
        }
    }

    pub(in crate::messaging::coordinator) fn settings_json(&self) -> Value {
        match self
            .settings_path
            .as_deref()
            .map(crate::messaging::prefs::settings::load)
        {
            Some(Ok(settings)) => json!(settings),
            Some(Err(error)) => json!({"error": error}),
            None => Value::Null,
        }
    }

    /// The optional `sound` parameter: None when absent, Some(None) for Bus's
    /// own ding, or an installed system sound by its listed name.
    pub(in crate::messaging::coordinator) fn sound_choice(
        &self,
        p: &Value,
    ) -> Result<Option<Option<String>>, String> {
        Ok(match optional_text(p, "sound")? {
            None => None,
            Some(name) if name.eq_ignore_ascii_case(crate::platform::sound::DEFAULT_SOUND_NAME) => {
                Some(None)
            }
            Some(name) => Some(Some(
                crate::platform::sound::find_sound(&self.system_sounds(), name)
                    .map(|sound| sound.name.clone())
                    .ok_or_else(|| format!("Unknown sound {name:?}; `bus sounds` lists them"))?,
            )),
        })
    }

    pub(in crate::messaging::coordinator) fn system_sounds(
        &self,
    ) -> Vec<crate::platform::sound::SystemSound> {
        match &self.sound_dirs {
            Some(dirs) => crate::platform::sound::list_sounds(dirs),
            None => crate::platform::sound::system_sounds(),
        }
    }

    pub(in crate::messaging::coordinator) fn dev_read(
        &mut self,
        id: AgentId,
        source: Option<&str>,
        lines: Option<u32>,
    ) -> Result<Value, String> {
        let read_source = dev_read_source(source, lines)?;
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
        let capture = read_capture(&read, read_source);
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
}

/// A `state` agent entry: the stored agent plus why a queued message would
/// wait on it, which orchestrators need now that `diagnostics` is dev tier.
fn agent_json(agent: &crate::messaging::model::RoomAgent) -> Value {
    let mut value = json!(agent);
    value["wait_reason"] = json!(crate::messaging::diagnostics::wait_reason(agent));
    value
}

/// How the running Bus was built: `debug` for a development build (`./run dev`,
/// `cargo build`), `release` for an optimized build, plus the binary's path.
fn build_json() -> Value {
    json!({
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "binary": std::env::current_exe().ok(),
    })
}

fn dev_read_source(source: Option<&str>, lines: Option<u32>) -> Result<schema::ReadSource, String> {
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
    Ok(read_source)
}

fn read_capture(read: &schema::PaneReadResult, read_source: schema::ReadSource) -> Value {
    let mut capture = json!({
        "at_ms": crate::messaging::storage::io::now_ms(),
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
    capture
}
