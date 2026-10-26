use super::{control_dispatch, mpsc, Arc, BusCommand, BusEvent, BusSnapshot, Mutex, Worker};
use crate::messaging::control::server::ControlCall;
use std::time::Instant;

impl Worker {
    pub(super) fn serve_control_without_refresh(
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
        read && self.revision == revision && self.error == error && Instant::now() < next_poll
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
