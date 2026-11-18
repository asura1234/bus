//! Bounded ordering of coordinator commands and native-delivery barriers.
use super::{control_dispatch, interrupts_delivery, mpsc, BusCommand};
use std::{collections::VecDeque, time::Duration};

/// How many commands may run ahead of a waiting status poll before it runs:
/// interactive calls skip the poll backlog, and polls still progress.
pub(super) const STATUS_POLL_OVERTAKES: usize = 4;

/// A status poll that has waited this long runs ahead of earlier reads and UI
/// commands. One composer keystroke queues a draft update, so a typing burst
/// can queue minutes of commands; the poll's caller gives up after 15 s.
pub(super) const STATUS_POLL_MAX_WAIT: Duration = Duration::from_secs(3);

fn is_status_poll(command: &BusCommand) -> bool {
    matches!(command, BusCommand::Control(call) if call.request.method == "message.status")
}

/// Mutations and shutdown keep their order: a read never jumps past one.
fn keeps_its_place(command: &BusCommand) -> bool {
    match command {
        BusCommand::Shutdown => true,
        BusCommand::Control(call) => control_dispatch::is_mutation(&call.request.method),
        _ => false,
    }
}

/// The oldest queued status poll when it must run before the work queued
/// ahead of it: it has waited [`STATUS_POLL_MAX_WAIT`], or it has already been
/// overtaken [`STATUS_POLL_OVERTAKES`] times. A mutation or shutdown ahead of
/// it keeps its place.
fn due_status_poll(pending: &VecDeque<(u64, BusCommand)>, overtakes: usize) -> Option<usize> {
    let poll = pending
        .iter()
        .position(|(_, command)| is_status_poll(command))?;
    if pending
        .iter()
        .take(poll)
        .any(|(_, command)| keeps_its_place(command))
    {
        return None;
    }
    let waited = match &pending[poll].1 {
        BusCommand::Control(call) => call.queued_for(),
        _ => None,
    };
    let aged = waited.is_some_and(|waited| waited >= STATUS_POLL_MAX_WAIT);
    (aged || overtakes >= STATUS_POLL_OVERTAKES).then_some(poll)
}

/// The next command to run: a status poll that is due (see [`due_status_poll`])
/// first, else the developer's oldest UI command when only control reads
/// (follower status polls) are queued ahead of it, else the oldest command
/// other than a status poll, else the oldest poll. Otherwise a waiting poll is
/// overtaken at most `STATUS_POLL_OVERTAKES` times in a row by any command,
/// counted in `overtakes`. Control mutations and shutdown are never overtaken.
pub(super) fn next_command(
    commands: &mpsc::Receiver<(u64, BusCommand)>,
    pending: &mut VecDeque<(u64, BusCommand)>,
    overtakes: &mut usize,
) -> Option<(u64, BusCommand)> {
    for _ in 0..super::super::COMMAND_QUEUE_CAPACITY {
        match commands.try_recv() {
            Ok(command) => pending.push_back(command),
            Err(_) => break,
        }
    }
    if let Some(index) = due_status_poll(pending, *overtakes) {
        let command = pending.remove(index)?;
        *overtakes = 0;
        return Some(command);
    }
    let ui = pending.iter().position(|(_, command)| match command {
        BusCommand::Control(call) => control_dispatch::is_mutation(&call.request.method),
        _ => true,
    });
    let chosen = match ui {
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
                    pending.remove(index)
                }
                _ => {
                    *overtakes = 0;
                    pending.pop_front()
                }
            }
        }
    };
    // Every command served ahead of a waiting poll counts toward its budget,
    // developer UI commands included: a deep UI backlog otherwise starves the
    // poll for as long as the loop takes to drain it.
    match &chosen {
        Some((_, command)) if is_status_poll(command) => *overtakes = 0,
        Some(_) if pending.iter().any(|(_, command)| is_status_poll(command)) => {
            *overtakes += 1;
        }
        _ => *overtakes = 0,
    }
    chosen
}

/// Reads are answered after this tick, without suppressing its delivery. A
/// mutation still returns to command dispatch first, in FIFO order. Bound the
/// drain by the input channel's capacity so a read flood cannot trap this loop.
pub(super) fn collect_delivery_commands(
    commands: &mpsc::Receiver<(u64, BusCommand)>,
    pending: &mut VecDeque<(u64, BusCommand)>,
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
