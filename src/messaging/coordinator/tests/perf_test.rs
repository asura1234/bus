use super::*;
use crate::messaging::perf;
use std::time::{Duration, Instant};

#[test]
fn coordinator_loop_commands_ticks_and_snapshots_are_timed() {
    let dir = crate::utils::test_temp::unique_temp_path("bus-coordinator-perf");
    let (handle, _commands) = BusHandle::start_for_test(dir.clone(), &BusState::new());
    handle
        .try_send(1, BusCommand::CreateRoom("perf".into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut finished = false;
    while !finished && Instant::now() < deadline {
        finished = matches!(
            handle.try_event(),
            Some(BusEvent::CommandFinished { command_id: 1, .. })
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(finished, "the command settles");
    // The worker polls agents every 500 ms.
    std::thread::sleep(Duration::from_millis(700));

    assert!(perf::COORDINATOR_LOOP.iterations().take().is_some());
    assert!(perf::COMMAND.take().is_some_and(|window| window.count >= 1));
    assert!(perf::SNAPSHOT.take().is_some());
    assert!(perf::TICK.take().is_some());
    assert!(
        perf::STORE_SAVE.take().is_some() || perf::STORE_JOURNAL.take().is_some(),
        "creating a room is persisted"
    );
    let _ = handle.try_send(2, BusCommand::Shutdown);
    let _ = std::fs::remove_dir_all(dir);
}
