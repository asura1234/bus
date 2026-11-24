//! Session-scoped sender identities with no agent runtime.
use super::{BusState, ModelError, RoomId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Bot {
    pub(crate) name: String,
    pub(crate) room_id: Option<RoomId>,
}

impl BusState {
    pub(crate) fn bots(&self) -> impl Iterator<Item = &Bot> {
        self.bots.values()
    }

    pub(crate) fn bot(&self, name: &str) -> Option<&Bot> {
        self.bots.get(name)
    }

    pub(crate) fn add_bot(
        &mut self,
        name: &str,
        room_id: Option<RoomId>,
    ) -> Result<Bot, ModelError> {
        let name = self.available_agent_name(name, None)?;
        if let Some(room) = room_id {
            if self
                .room(room)
                .ok_or(ModelError::UnknownRoom(room))?
                .deletion_pending
            {
                return Err(ModelError::DeletionPending);
            }
        }
        let bot = Bot { name, room_id };
        self.bots.insert(bot.name.clone(), bot.clone());
        Ok(bot)
    }

    pub(crate) fn delete_bot(&mut self, name: &str) -> Result<Bot, ModelError> {
        self.bots
            .remove(name)
            .ok_or_else(|| ModelError::UnknownBot(name.to_owned()))
    }

    pub(crate) fn author_label(&self, author: &super::Author) -> String {
        match author {
            super::Author::Developer => "You".into(),
            super::Author::Bus => super::BUS_SENDER.into(),
            super::Author::Agent(id) => self.agent_label(*id),
            super::Author::Bot(name) => name.clone(),
        }
    }
}
