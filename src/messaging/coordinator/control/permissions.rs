//! Typed decisions and lease checks share the coordinator's single writer.
use super::{optional_text, required, AgentId, Worker};
use crate::messaging::model::PermissionOutcome;
use crate::messaging::storage::io;
use serde_json::{json, Value};
impl Worker {
    pub(super) fn execute_permission_decision(&mut self, p: &Value) -> Result<Value, String> {
        let worker = self.control_agent(required(p, "agent")?, None)?;
        let handler = self.control_agent(required(p, "handler")?, None)?;
        self.state.validate_permission_lease(
            worker,
            handler,
            required(p, "assignment")?,
            io::now_ms(),
        )?;
        let outcome = match required(p, "outcome")? {
            "approved" => PermissionOutcome::Approved,
            "escalated" => PermissionOutcome::Escalated,
            "routed" => PermissionOutcome::Routed,
            _ => return Err("Outcome must be approved, escalated, or routed".into()),
        };
        let fingerprint = required(p, "fingerprint")?;
        if fingerprint != "unavailable" {
            self.validate_permission_fingerprint(worker, fingerprint)?;
        }
        if outcome == PermissionOutcome::Approved && fingerprint == "unavailable" {
            return Err("Approval requires a fingerprint".into());
        }
        if self
            .state
            .permission_assignment(worker)
            .is_some_and(|a| a.answer_attempted && a.fingerprint.as_deref() != Some(fingerprint))
        {
            return Err("Decision fingerprint differs from the typed answer".into());
        }
        let mut state = self.state.clone();
        if fingerprint != "unavailable" {
            state.permission_record_fingerprint(worker, fingerprint);
        }
        state.complete_permission(worker, outcome, required(p, "summary")?, io::now_ms())?;
        self.save(state)?;
        Ok(json!({"agent_id":worker,"outcome":outcome,"recorded":true}))
    }
    pub(super) fn managed_dialog_lease(&self, worker: AgentId, p: &Value) -> Result<bool, String> {
        match (
            optional_text(p, "handler")?,
            optional_text(p, "assignment")?,
        ) {
            (None, None) => {
                if self
                    .state
                    .permission_assignment(worker)
                    .is_some_and(|a| a.outcome.is_none())
                {
                    return Err(
                        "Active permission assignment requires --handler and --assignment".into(),
                    );
                }
                Ok(false)
            }
            (Some(handler), Some(token)) => {
                self.state.validate_permission_lease(
                    worker,
                    self.control_agent(handler, None)?,
                    token,
                    io::now_ms(),
                )?;
                Ok(true)
            }
            _ => Err("--handler and --assignment must be supplied together".into()),
        }
    }
    pub(super) fn managed_dialog_result(
        &mut self,
        worker: AgentId,
        result: &Result<Value, String>,
    ) -> Result<(), String> {
        let confirmed = result
            .as_ref()
            .is_ok_and(|v| matches!(v["outcome"].as_str(), Some("closed" | "replaced")));
        if confirmed {
            let mut state = self.state.clone();
            state.permission_answer_confirm(worker);
            self.save(state)
        } else {
            self.permission_fallback(
                worker,
                "typed answer outcome uncertain or unconfirmed; do not retry",
            )
        }
    }
}
