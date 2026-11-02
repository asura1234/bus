//! An internal save-and-quit request, authorized by the CLI's published handoff.
use super::{mpsc, BusEvent, Worker};
use crate::messaging::control::{Request, Response};
use crate::utils::restart_record::{self, HandoffPlan};
use std::sync::Mutex;

// The normal quit shuts the coordinator down before main relaunches. Keep the
// validated plan in this process instead of reading a mutable file after quit.
static RESTART_PLAN: Mutex<Option<HandoffPlan>> = Mutex::new(None);

pub(crate) fn take_restart_plan() -> Option<HandoffPlan> {
    RESTART_PLAN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
}

#[cfg(test)]
pub(crate) fn test_restart_plan(plan: HandoffPlan) {
    *RESTART_PLAN
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(plan);
}

#[cfg(test)]
#[path = "tests/restart_handoff_test.rs"]
mod tests;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RestartQuit {
    helper_pid: u32,
    plan: HandoffPlan,
}

impl Worker {
    pub(super) fn restart_quit_response(
        &self,
        request: &Request,
        events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Response {
        match self.authorize_restart_quit(&request.params, events) {
            Ok(()) => Response::success(&request.id, serde_json::json!({"stage":"queued"})),
            Err(error) => Response::failure(&request.id, "command_failed", error),
        }
    }

    fn authorize_restart_quit(
        &self,
        params: &serde_json::Value,
        events: Option<&mpsc::Sender<BusEvent>>,
    ) -> Result<(), String> {
        if params.as_object().is_none_or(|fields| fields.len() != 1) {
            return Err("invalid internal restart quit parameters".into());
        }
        let quit: RestartQuit = serde_json::from_value(params["restart"].clone())
            .map_err(|error| format!("invalid internal restart quit: {error}"))?;
        let plan = quit.plan;
        let helper =
            restart_record::read_live_pid(&restart_record::restart_lock_path(&self.data_dir))
                .ok_or("restart helper has no lock")?;
        let pending = restart_record::read_handoff(&restart_record::handoff_path(&self.data_dir))?
            .ok_or("restart handoff is missing")?;
        if plan != pending
            || plan.root != self.data_dir
            || plan.tui.pid != std::process::id()
            || !crate::platform::process_still_live(plan.tui.pid, plan.tui.birth)
            || helper.pid != quit.helper_pid
            || !crate::platform::process_still_live(helper.pid, helper.birth)
        {
            return Err(
                "restart quit does not match the published handoff and live processes".into(),
            );
        }
        let events = events.ok_or("Bus UI event channel unavailable")?;
        *RESTART_PLAN
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(plan);
        if events.send(BusEvent::DevQuitRequested).is_err() {
            let _ = take_restart_plan();
            return Err("Bus UI event channel disconnected".into());
        }
        Ok(())
    }
}
