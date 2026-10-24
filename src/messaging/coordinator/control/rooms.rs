//! Rooms development commands on the single coordinator.
use super::{
    mpsc, optional_text, required, unique, BTreeMap, BusCommand, BusEvent, RoomId, Worker,
};
use serde_json::{json, Value};

impl Worker {
    pub(super) fn execute_rooms(
        &mut self,
        method: &str,
        p: &Value,
        events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<Value, String> {
        match method {
            "agent.focus" | "room.focus" => {
                let (room, agent) = if method == "agent.focus" {
                    let id = self.control_agent(p)?;
                    let agent = self.state.agent(id).ok_or("Unknown agent")?;
                    if agent.runtime_identity.pane_id.is_none() {
                        return Err("Agent has no terminal; inspect its launch error".into());
                    }
                    (agent.room_id, Some(id))
                } else {
                    (self.control_room(required(p, "room")?)?, None)
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
            "room.create" => {
                self.control_command(BusCommand::CreateRoom(required(p, "name")?.into()))
            }
            "room.rename" => self.control_command(BusCommand::RenameRoom(
                self.control_room(required(p, "room")?)?,
                required(p, "name")?.into(),
            )),
            "room.notes" => self.control_command(BusCommand::SetNotes(
                self.control_room(required(p, "room")?)?,
                optional_text(p, "text")?.ok_or("Missing text")?.into(),
            )),
            "room.seen" => self.control_command(BusCommand::MarkRoomSeen(
                self.control_room(required(p, "room")?)?,
            )),
            "room.sound" => {
                let room = self.control_room(required(p, "room")?)?;
                let on = p
                    .get("on")
                    .and_then(Value::as_bool)
                    .ok_or("Sound must be on or off")?;
                // Resolve the name before changing anything, so an unknown
                // sound leaves the room as it was.
                let name = self.sound_choice(p)?;
                if let Some(name) = name {
                    self.control_command(BusCommand::SetRoomSoundName(room, name))?;
                }
                self.control_command(BusCommand::SetRoomSound(room, on))
            }
            "room.delete" => self.control_command(BusCommand::DeleteRoom(
                self.control_room(required(p, "room")?)?,
            )),
            "room.history" => self.control_history(required(p, "room")?),
            _ => Err("Unknown method".into()),
        }
    }

    fn control_history(&mut self, selector: &str) -> Result<Value, String> {
        let room = self.control_history_room(selector)?;
        let now = super::super::io::now_ms();
        self.expire_queued_requests(now)?;
        // Group once: constructing each delivery must not rescan all history.
        let mut deliveries = BTreeMap::<_, Vec<_>>::new();
        for request in self.state.requests() {
            deliveries
                .entry(request.prompt.id)
                .or_default()
                .push(request);
        }
        // Messages carry their delivery; notices (reports to the
        // developer, interrupts) were delivered to nobody.
        let mut messages = self
            .state
            .requests()
            .filter(|r| r.room_id == room)
            .map(|r| (r.prompt.id, (&r.prompt, true)))
            .collect::<BTreeMap<_, _>>();
        messages.extend(self.state.notices(room).map(|n| (n.id, (n, false))));
        Ok(
            json!({"room_id":room,"room_name":self.state.history_room_name(room),"deleted":self.state.room(room).is_none(),"messages":messages.iter().map(|(id,(prompt,delivered))|json!({"prompt":prompt,"author_name":match &prompt.author { crate::messaging::model::Author::Agent(agent) => self.state.agent_label(*agent), crate::messaging::model::Author::Developer => "You".into(), crate::messaging::model::Author::Bus => "bus".into() },"recipient_names":prompt.recipient_ids.iter().map(|id| self.state.agent_label(*id)).collect::<Vec<_>>(),"delivery":delivered.then(|| self.control_message_from(*id, now, deliveries.get(id).cloned().unwrap_or_default()).ok()).flatten()})).collect::<Vec<_>>()}),
        )
    }

    fn control_history_room(&self, selector: &str) -> Result<RoomId, String> {
        if let Ok(id) = selector.parse::<u64>().map(RoomId) {
            if self.state.history_room_name(id).is_some()
                || self.state.requests().any(|r| r.room_id == id)
                || self.state.notices(id).next().is_some()
            {
                return Ok(id);
            }
        }
        if selector.eq_ignore_ascii_case(crate::messaging::model::MASTER_ROOM_NAME)
            || self.state.rooms().any(|r| r.name == selector)
        {
            return self.control_room(selector);
        }
        unique(
            self.state
                .history_rooms()
                .filter(|(_, name)| *name == selector)
                .map(|(id, _)| id),
        )
    }

    pub(in crate::messaging::coordinator) fn control_room(
        &self,
        selector: &str,
    ) -> Result<RoomId, String> {
        if selector.eq_ignore_ascii_case(crate::messaging::model::MASTER_ROOM_NAME) {
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
}
