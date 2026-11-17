//! Explicit bridge ingress never resolves a pane author or settles an agent request.
use super::{required, Worker};
use crate::messaging::{
    feishu::{ingest::Message, Client},
    prefs::settings,
};
use serde_json::Value;

impl Worker {
    pub(super) fn control_feishu_ingest(&mut self, params: &Value) -> Result<Value, String> {
        let _log_only = crate::utils::error_pipe::suppress(true);
        let result = self.ingest_feishu(params);
        if let Err(error) = &result {
            tracing::warn!(event = "bus.feishu.ingest_refused", %error, "Feishu input was not accepted");
        }
        result
    }

    fn ingest_feishu(&mut self, params: &Value) -> Result<Value, String> {
        if self.batch.is_some() {
            return Err("Feishu ingest must wait for the current state transaction".into());
        }
        let path = self
            .settings_path
            .as_deref()
            .ok_or("Feishu settings are unavailable")?;
        let settings = settings::load(path)?;
        if Client::default()
            .program(settings.feishu_bridge_path.as_deref())
            .is_none()
        {
            return Err("Feishu bridge is not installed".into());
        }
        let message: Message =
            serde_json::from_value(params.get("event").cloned().ok_or("Missing Feishu event")?)
                .map_err(|_| "Invalid Feishu bridge v1 message")?;
        let input = message.normalize(required(params, "app_id")?)?;
        if let Ok(record) = self.state.feishu.resolve(&input) {
            let original = std::fs::canonicalize(&record.session)
                .map_err(|_| "Recorded Feishu Bus session is unavailable")?;
            let current = std::fs::canonicalize(&self.data_dir)
                .map_err(|_| "Current Bus session is unavailable")?;
            if original != current {
                return Err("Feishu reply belongs to a different Bus session".into());
            }
        }
        let mut state = self.state.clone();
        let known: std::collections::BTreeSet<_> =
            self.state.requests().map(|request| request.id).collect();
        let now = crate::messaging::storage::io::now_ms();
        let outcome = state.ingest_feishu_reply(input, &settings.feishu_recipient, now)?;
        // A Feishu reply is the developer's message. Every request this ingest
        // created interrupts its recipient's running turn and runs next, like
        // every other developer path. Alias and parked resolutions created by
        // this ingest are included.
        let created: Vec<_> = state
            .requests()
            .filter(|request| {
                !known.contains(&request.id)
                    && request.prompt.author == crate::messaging::model::Author::Developer
            })
            .map(|request| request.id)
            .collect();
        for id in created {
            let Some((room, recipients)) = state
                .request(id)
                .map(|request| (request.room_id, request.prompt.recipient_ids.clone()))
            else {
                continue;
            };
            self.interrupt_queued_message(
                &mut state,
                room,
                &recipients,
                &crate::messaging::model::Author::Developer,
                now,
            );
        }
        self.save(state)?;
        serde_json::to_value(outcome).map_err(|error| error.to_string())
    }
}
