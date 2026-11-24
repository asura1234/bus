//! Bot identity commands share the coordinator's durable single writer.
use super::{optional_text, required, Worker};
use serde_json::{json, Value};

impl Worker {
    pub(super) fn execute_bots(&mut self, method: &str, p: &Value) -> Result<Value, String> {
        match method {
            "bot.add" => {
                let room = optional_text(p, "room")?
                    .map(|room| self.control_room(room))
                    .transpose()?;
                let mut state = self.state.clone();
                let bot = state
                    .add_bot(required(p, "name")?, room)
                    .map_err(|error| error.to_string())?;
                self.save(state)?;
                Ok(json!(bot))
            }
            "bot.list" => Ok(json!({"bots":self.bots_json()})),
            "bot.delete" => {
                let mut state = self.state.clone();
                let bot = state
                    .delete_bot(required(p, "name")?)
                    .map_err(|error| error.to_string())?;
                self.save(state)?;
                Ok(json!({"name":bot.name,"deleted":true}))
            }
            _ => Err("Unknown method".into()),
        }
    }

    pub(super) fn bots_json(&self) -> Value {
        json!(self.state.bots().collect::<Vec<_>>())
    }
}
