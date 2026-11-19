use super::{control_dispatch, mpsc, Arc, BusCommand, BusEvent, BusSnapshot, Mutex, Worker};
use crate::messaging::control::server::ControlCall;
use std::time::Instant;

impl Worker {
    pub(in crate::messaging::coordinator) fn serve_control_without_refresh(
        &mut self,
        call: ControlCall,
        events: &mpsc::Sender<BusEvent>,
        snapshots: &Arc<Mutex<Arc<BusSnapshot>>>,
        next_poll: Instant,
    ) -> bool {
        let read = !control_dispatch::is_mutation(&call.request.method);
        let revision = self.revision;
        let error = self.error.clone();
        self.serve_control(call, events, snapshots);
        // Unchanged reads need no notification scan or copied UI state.
        // Due polls still run, so a follower flood cannot stop delivery.
        read && self.native_submissions.is_empty()
            && self.callback_wakes.is_empty()
            && self.revision == revision
            && self.error == error
            && Instant::now() < next_poll
    }
}

/// Preserve the old writer-before-revocation order without serializing sends
/// to unrelated recipients. These controls can revoke, replace or edit input.
pub(super) fn native_input_barrier(command: &BusCommand) -> bool {
    match command {
        BusCommand::DeleteAgent(_) | BusCommand::DeleteRoom(_) | BusCommand::Shutdown => true,
        BusCommand::Control(call) => matches!(
            call.request.method.as_str(),
            "agent.interrupt"
                | "agent.clear"
                | "agent.resume"
                | "agent.delete"
                | "agent.press"
                | "room.delete"
                | "request.recover"
        ),
        _ => false,
    }
}

pub(super) fn interrupts_delivery(command: &BusCommand) -> bool {
    match command {
        BusCommand::Control(call) => control_dispatch::is_mutation(&call.request.method),
        _ => true,
    }
}

#[cfg(test)]
#[path = "../tests/control_read_error_test.rs"]
mod tests;
