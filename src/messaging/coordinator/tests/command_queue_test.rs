//! Status polls must not lose their caller's deadline behind queued work.
use super::super::{mpsc, BusCommand, RoomId};
use super::command_queue::{next_command, STATUS_POLL_MAX_WAIT, STATUS_POLL_OVERTAKES};
use crate::messaging::control::{server::ControlCall, Request};
use serde_json::json;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// A follower's `message.status` read. Only a transport-dispatched call
/// carries a deadline; direct callers leave it out.
fn poll(id: u64, waited: Duration) -> (u64, BusCommand) {
    let (reply, _) = mpsc::sync_channel(1);
    let deadline =
        Some(Instant::now() + Duration::from_secs(15) - waited.min(Duration::from_secs(14)));
    (
        id,
        BusCommand::Control(ControlCall {
            request: Request {
                id: format!("poll-{id}"),
                method: "message.status".into(),
                params: json!({"message": "1"}),
            },
            reply,
            deadline,
            _poll_permit: None,
            form_generation: None,
        }),
    )
}

/// One composer keystroke: a developer UI command with no deadline.
fn draft(id: u64) -> (u64, BusCommand) {
    (
        id,
        BusCommand::SetDraftText(RoomId(1), format!("draft {id}")),
    )
}

/// A control mutation: its order against reads is fixed.
fn mutation(id: u64) -> (u64, BusCommand) {
    let (reply, _) = mpsc::sync_channel(1);
    (
        id,
        BusCommand::Control(ControlCall {
            request: Request {
                id: format!("mutation-{id}"),
                method: "room.create".into(),
                params: json!({"name": "first"}),
            },
            reply,
            deadline: None,
            _poll_permit: None,
            form_generation: None,
        }),
    )
}

fn queued(
    commands: impl IntoIterator<Item = (u64, BusCommand)>,
) -> mpsc::Receiver<(u64, BusCommand)> {
    let (sender, receiver) = mpsc::sync_channel(64);
    for command in commands {
        sender.send(command).unwrap();
    }
    receiver
}

fn serve(
    commands: &mpsc::Receiver<(u64, BusCommand)>,
    pending: &mut VecDeque<(u64, BusCommand)>,
    overtakes: &mut usize,
    count: usize,
) -> Vec<u64> {
    (0..count)
        .filter_map(|_| next_command(commands, pending, overtakes))
        .map(|(id, _)| id)
        .collect()
}

#[test]
fn a_status_poll_runs_within_the_overtake_budget_behind_ui_commands() {
    let mut commands = vec![poll(1, Duration::ZERO)];
    commands.extend((2..=20).map(draft));
    let commands = queued(commands);
    let mut pending = VecDeque::new();
    let mut overtakes = 0;
    let served = serve(&commands, &mut pending, &mut overtakes, 20);
    let poll_at = served.iter().position(|id| *id == 1).expect("the poll ran");
    assert!(
        poll_at <= STATUS_POLL_OVERTAKES,
        "a poll behind a UI-command burst waited {poll_at} commands: {served:?}"
    );
}

#[test]
fn an_aged_status_poll_runs_before_earlier_ui_commands() {
    let commands = queued(vec![
        draft(2),
        draft(3),
        poll(1, STATUS_POLL_MAX_WAIT),
        draft(4),
    ]);
    let mut pending = VecDeque::new();
    let mut overtakes = 0;
    let served = serve(&commands, &mut pending, &mut overtakes, 1);
    assert_eq!(
        served,
        [1],
        "a poll whose caller has waited {STATUS_POLL_MAX_WAIT:?} must not queue behind UI commands"
    );
}

#[test]
fn a_status_poll_does_not_jump_a_mutation_queued_before_it() {
    let commands = queued(vec![mutation(2), poll(1, STATUS_POLL_MAX_WAIT), draft(3)]);
    let mut pending = VecDeque::new();
    let mut overtakes = STATUS_POLL_OVERTAKES;
    let served = serve(&commands, &mut pending, &mut overtakes, 2);
    assert_eq!(
        served,
        [2, 1],
        "the mutation keeps its place; the poll runs next"
    );
}

#[test]
fn a_status_poll_does_not_jump_a_shutdown_queued_before_it() {
    let commands = queued(vec![
        (9, BusCommand::Shutdown),
        poll(1, STATUS_POLL_MAX_WAIT),
    ]);
    let mut pending = VecDeque::new();
    let mut overtakes = STATUS_POLL_OVERTAKES;
    let served = serve(&commands, &mut pending, &mut overtakes, 2);
    assert_eq!(
        served,
        [9, 1],
        "shutdown keeps its place; the poll is still answered after it"
    );
}

#[test]
fn each_status_poll_gets_its_own_overtake_budget() {
    let mut commands = vec![poll(1, Duration::ZERO), poll(2, Duration::ZERO)];
    commands.extend((3..=20).map(draft));
    let commands = queued(commands);
    let mut pending = VecDeque::new();
    let mut overtakes = 0;
    let served = serve(&commands, &mut pending, &mut overtakes, 20);
    let first = served.iter().position(|id| *id == 1).expect("first poll");
    let second = served.iter().position(|id| *id == 2).expect("second poll");
    assert_eq!(
        (first, second),
        (STATUS_POLL_OVERTAKES, 2 * STATUS_POLL_OVERTAKES + 1),
        "the second poll waits for its own budget, not the whole draft burst: {served:?}"
    );
}
