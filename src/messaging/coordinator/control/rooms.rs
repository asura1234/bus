//! Rooms development commands on the single coordinator.
use super::{mpsc, required, unique, BTreeMap, BusCommand, BusEvent, RoomId, Worker};
use crate::messaging::model::{Author, PromptId, Request};
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
                    .send(BusEvent::DevFocusRequested {
                        room,
                        agent,
                        form_generation: Some(
                            self.queued_focus_generation
                                .unwrap_or_else(super::super::current_form_generation),
                        ),
                    })
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
            "room.history" => self.control_history(p),
            _ => Err("Unknown method".into()),
        }
    }

    fn control_history(&mut self, p: &Value) -> Result<Value, String> {
        let room = self.control_history_room(required(p, "room")?)?;
        let positive = |name: &str| -> Result<Option<u64>, String> {
            p.get(name)
                .map(|value| {
                    value
                        .as_u64()
                        .filter(|value| *value > 0)
                        .ok_or_else(|| format!("History {name} must be a positive integer"))
                })
                .transpose()
        };
        let limit = positive("limit")?;
        let before = positive("before")?;
        if before.is_some() && limit.is_none() {
            return Err("History before requires limit".into());
        }
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
        if let Some(before) = before {
            messages.retain(|id, _| id.0 < before);
        }
        let skip = limit.map_or(0, |limit| {
            messages
                .len()
                .saturating_sub(usize::try_from(limit).unwrap_or(usize::MAX))
        });
        let next_before = (skip > 0)
            .then(|| messages.keys().nth(skip).copied())
            .flatten();
        let mut result = json!({"room_id":room,"room_name":self.state.history_room_name(room),"deleted":self.state.room(room).is_none(),"messages":messages.iter().skip(skip).map(|(id,(prompt,delivered))|json!({"prompt":prompt,"author_name":self.state.author_label(&prompt.author),"recipient_names":prompt.recipient_ids.iter().map(|id| self.state.agent_label(*id)).collect::<Vec<_>>(),"delivery":delivered.then(|| self.history_delivery(*id, now, deliveries.get(id).cloned().unwrap_or_default())).flatten()})).collect::<Vec<_>>()});
        if limit.is_some() {
            result["has_more"] = json!(skip > 0);
            result["next_before"] = json!(next_before);
        }
        Ok(result)
    }

    /// Attach developer answers to their message. Message status remains lossless.
    fn history_delivery(
        &self,
        message: PromptId,
        now: u64,
        requests: Vec<&Request>,
    ) -> Option<Value> {
        let visible_replies: std::collections::BTreeSet<_> = requests
            .iter()
            .filter(|request| self.state.developer_history_reply(request).is_some())
            .map(|request| request.id.0)
            .collect();
        let hidden_reply_reason = match requests.first().map(|request| &request.prompt.author) {
            Some(Author::Agent(_)) => "requester_reply",
            Some(Author::Developer) => "shared_reply",
            Some(Author::Bot(_) | Author::Bus) | None => "internal_reply",
        };
        let mut delivery = self.control_message_from(message, now, requests).ok()?;
        for request in delivery.get_mut("requests")?.as_array_mut()? {
            let Some(object) = request.as_object_mut() else {
                continue;
            };
            let visible = object
                .get("request_id")
                .and_then(Value::as_u64)
                .is_some_and(|id| visible_replies.contains(&id));
            if !visible && object.get("stage").and_then(Value::as_str) == Some("replied") {
                object.insert("reply".into(), Value::Null);
                object.insert("stage".into(), json!("ended"));
                let reason_missing = object
                    .get("reason")
                    .and_then(Value::as_str)
                    .is_none_or(|reason| reason.trim().is_empty());
                if reason_missing {
                    object.insert("reason".into(), json!(hidden_reply_reason));
                }
            }
            object.insert("group".into(), Value::Null);
            object.remove("answered_with_message");
            object.insert("reply_ack".into(), Value::Bool(false));
        }
        Some(delivery)
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
