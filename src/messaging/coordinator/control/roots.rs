//! Explicit root grants and scratch-only historical registration fixtures.
use super::{required, Worker};
use serde_json::{json, Value};

impl Worker {
    pub(super) fn control_allow_root(&mut self, p: &Value) -> Result<Value, String> {
        let worker = self.control_agent(required(p, "agent")?, None)?;
        let by = self.control_agent(required(p, "by")?, None)?;
        let input = std::path::PathBuf::from(required(p, "path")?);
        if !input.is_absolute() {
            return Err("Allowed root must be absolute".into());
        }
        let root = input
            .canonicalize()
            .map_err(|e| format!("Allowed root unavailable: {e}"))?;
        if !root.is_dir() {
            return Err("Allowed root must be a directory".into());
        }
        let hooks = crate::agents::providers::hook_json::HookContext {
            binary: std::env::current_exe().map_err(|e| e.to_string())?,
            spool: self.data_dir.join("callbacks/root-grant"),
            launch_id: String::new(),
        };
        if crate::agents::providers::launch::profile::protected_paths(&hooks)
            .iter()
            .any(|p| p.starts_with(&root) || root.starts_with(p))
        {
            return Err("Allowed root overlaps live Bus resources".into());
        }
        let mut state = self.state.clone();
        state.grant_agent_root(
            worker,
            by,
            root.clone(),
            crate::messaging::storage::io::now_ms(),
        )?;
        self.save(state)?;
        Ok(
            json!({"agent_id":worker,"allowed_root":root,"granted_by":by,"sandbox_update":"applies on next native resume; handler authority applies immediately"}),
        )
    }

    #[cfg(feature = "dev-tools")]
    pub(super) fn control_legacy_fixture(&mut self, p: &Value) -> Result<Value, String> {
        if std::env::var_os("BUS_TUI_FAKE_AGENT").is_none() {
            return Err("Legacy fixture requires a scratch fake-provider session".into());
        }
        let worker = self.control_agent(required(p, "agent")?, None)?;
        let agent = self.state.agent(worker).ok_or("Unknown worker")?;
        if agent.permission_handler || agent.orchestrates.is_some() || agent.dialog {
            return Err("Legacy fixture requires an ordinary worker without a dialog".into());
        }
        // Reconstruct the historical registration shape, then run the
        // same deserializer as JsonStore load. Never seed a decision.
        let mut value = serde_json::to_value(&self.state).map_err(|e| e.to_string())?;
        let saved = value["agents"][worker.0.to_string()]
            .as_object_mut()
            .ok_or("Missing worker")?;
        saved.remove("launch_cwd");
        saved.remove("launch_cwd_backfilled");
        let mut state: crate::messaging::model::BusState =
            serde_json::from_value(value).map_err(|e| e.to_string())?;
        state = state.with_history(std::sync::Arc::clone(self.state.history()));
        self.save(state)?;
        Ok(json!({"agent_id":worker,"legacy_state_reloaded":true}))
    }
}
