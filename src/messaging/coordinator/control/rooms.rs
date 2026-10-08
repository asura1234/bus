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
            "room.delete" => {
                self.dev_command(BusCommand::DeleteRoom(self.dev_room(required(p, "room")?)?))
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

    pub(in crate::messaging::coordinator) fn dev_room(
        &self,
        selector: &str,
    ) -> Result<RoomId, String> {
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
}
