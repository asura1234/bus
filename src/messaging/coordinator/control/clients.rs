//! Attached UI clients share domain state but never selection or draft state.
use super::{required, BusCommand, BusEvent, Worker};
use serde_json::{json, Value};
use std::sync::mpsc;

impl Worker {
    pub(super) fn execute_client(&mut self, method: &str, p: &Value) -> Result<Value, String> {
        let client = required(p, "client_id")?;
        if client.len() > 128 || client.chars().any(char::is_control) {
            return Err("Invalid client identity".into());
        }
        #[cfg(feature = "dev-tools")]
        if p["local_client"].as_bool() == Some(true) {
            super::ui_hits::receive_client_state(&p["ui_state"]);
        }
        let mut events = Vec::new();
        if method == "client.command" {
            let id = p["command_id"]
                .as_u64()
                .filter(|id| *id > 0 && *id < 1 << 63)
                .ok_or("Invalid client command ID")?;
            let command: BusCommand = serde_json::from_value(p["command"].clone())
                .map_err(|error| format!("Invalid client command: {error}"))?;
            // These belong to the viewing client, never to the shared Bus.
            if matches!(
                command,
                BusCommand::SelectRoom(_)
                    | BusCommand::LeaveRoom
                    | BusCommand::MarkRoomSeen(_)
                    | BusCommand::SetDraftText(..)
                    | BusCommand::SetRecipients(..)
                    | BusCommand::AttachFile(..)
                    | BusCommand::RemoveFile(..)
                    | BusCommand::Control(_)
                    | BusCommand::CallbackReady(_)
                    | BusCommand::Shutdown
            ) {
                return Err("This command belongs to the viewing client".into());
            }
            let (tx, rx) = mpsc::channel();
            let previous = self.active_command_id;
            self.active_command_id = id;
            // Focus is connection-local; return only the pane identity and let
            // that client's native connection focus it.
            let result = match command {
                BusCommand::Submit(room, draft) => {
                    self.persist_client_draft_submission(room, draft, false)
                }
                BusCommand::SubmitQueued(room, draft) => {
                    self.persist_client_draft_submission(room, draft, true)
                }
                BusCommand::FocusTerminal(agent) => self
                    .state
                    .agent(agent)
                    .and_then(|agent| agent.runtime_identity.pane_id.clone())
                    .ok_or_else(|| "Agent has no terminal; inspect its launch error".to_owned())
                    .map(|pane_id| {
                        let _ = tx.send(BusEvent::TerminalFocused { agent, pane_id });
                    }),
                command => self.command(command, &tx),
            };
            self.active_command_id = previous;
            events.extend(rx.try_iter());
            events.push(BusEvent::CommandFinished {
                command_id: id,
                result,
            });
        }
        if method == "client.snapshot" {
            events.extend(crate::messaging::service::client_events(
                client,
                p["local_client"].as_bool() == Some(true),
            ));
        }
        let mut result = json!({"events":events});
        #[cfg(feature = "dev-tools")]
        if p["local_client"].as_bool() == Some(true) {
            result["ui_ack_hold"] = json!(super::ui_hits::ack_holding());
        }
        if method == "client.snapshot" {
            if let Some(viewer) = p["viewer_id"].as_str() {
                let routes = serde_json::from_value(p["routes"].clone()).unwrap_or_default();
                let completions =
                    serde_json::from_value(p["completions"].clone()).unwrap_or_default();
                result["forward_jobs"] = json!(crate::messaging::machine_routes::exchange(
                    viewer,
                    routes,
                    completions
                ));
            }
        }
        self.append_client_snapshot(&mut result);
        Ok(result)
    }

    pub(super) fn append_client_snapshot(&self, result: &mut Value) {
        let mut snapshot = self.snapshot();
        snapshot.state = snapshot.state.machine_client_projection();
        result["snapshot"] = json!(snapshot);
        result["machine_id"] =
            json!(crate::messaging::machine_routes::machine_identity(&self.data_dir).ok());
        result["history"] = json!(self.state.history().client_window(100, 4 * 1024 * 1024));
    }
}
