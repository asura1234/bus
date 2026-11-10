//! Bounded ordering of coordinator commands and native-delivery barriers.
use super::{control_dispatch, interrupts_delivery, mpsc, BusCommand};

/// How many commands may run ahead of a waiting status poll before it runs:
/// interactive calls skip the poll backlog, and polls still progress.
const STATUS_POLL_OVERTAKES: usize = 4;

/// The next command to run: the developer's oldest UI command when only
/// control reads (follower status polls) are queued ahead of it, else the
/// oldest command other than a status poll, else the oldest poll. A waiting
/// poll is overtaken at most `STATUS_POLL_OVERTAKES` times in a row, counted
/// in `overtakes`. Control mutations and shutdown are never overtaken.
pub(super) fn next_command(
    commands: &mpsc::Receiver<(u64, BusCommand)>,
    pending: &mut std::collections::VecDeque<(u64, BusCommand)>,
    overtakes: &mut usize,
) -> Option<(u64, BusCommand)> {
    for _ in 0..super::super::COMMAND_QUEUE_CAPACITY {
        match commands.try_recv() {
            Ok(command) => pending.push_back(command),
            Err(_) => break,
        }
    }
    let ui = pending.iter().position(|(_, command)| match command {
        BusCommand::Control(call) => control_dispatch::is_mutation(&call.request.method),
        _ => true,
    });
    match ui {
        Some(index)
            if !matches!(
                pending[index].1,
                BusCommand::Control(_) | BusCommand::CallbackReady(_) | BusCommand::Shutdown
            ) =>
        {
            pending.remove(index)
        }
        _ => {
            // Followers poll status by the hundred; a caller's one read,
            // interrupt or delete must not wait behind them. Shutdown keeps
            // its place, so polls queued before it are still answered.
            let interactive = pending.iter().position(|(_, command)| match command {
                BusCommand::Control(call) => call.request.method != "message.status",
                _ => true,
            });
            match interactive {
                Some(0) => pending.pop_front(),
                Some(index)
                    if !matches!(pending[index].1, BusCommand::Shutdown)
                        && *overtakes < STATUS_POLL_OVERTAKES =>
                {
                    *overtakes += 1;
                    pending.remove(index)
                }
                _ => {
                    *overtakes = 0;
                    pending.pop_front()
                }
            }
        }
    }
}

/// Reads are answered after this tick, without suppressing its delivery. A
/// mutation still returns to command dispatch first, in FIFO order. Bound the
/// drain by the input channel's capacity so a read flood cannot trap this loop.
pub(super) fn collect_delivery_commands(
    commands: &mpsc::Receiver<(u64, BusCommand)>,
    pending: &mut std::collections::VecDeque<(u64, BusCommand)>,
) -> bool {
    if pending
        .iter()
        .any(|(_, command)| interrupts_delivery(command))
    {
        return false;
    }
    for _ in 0..super::super::COMMAND_QUEUE_CAPACITY {
        match commands.try_recv() {
            Ok(command) => {
                let barrier = interrupts_delivery(&command.1);
                pending.push_back(command);
                if barrier {
                    return false;
                }
            }
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => return false,
        }
    }
    true
}
