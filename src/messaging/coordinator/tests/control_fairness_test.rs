use super::*;

#[test]
fn timed_out_controls_preserve_send_interrupt_delete_order() {
    let past = Instant::now() - Duration::from_secs(1);
    let order = run_queued("mutation-order", |reply| {
        vec![
            call(
                "poll",
                "message.status",
                json!({"message":"1"}),
                None,
                reply,
            ),
            call("send", "message.send", json!({}), Some(past), reply),
            call("interrupt", "agent.interrupt", json!({}), Some(past), reply),
            call("delete", "agent.delete", json!({}), Some(past), reply),
        ]
    });
    assert_eq!(order, ["send", "interrupt", "delete", "poll"]);
}

#[test]
fn status_polls_make_progress_while_interactive_reads_keep_arriving() {
    let dir = data_dir("poll-fairness");
    let mut worker = Worker::open(dir.clone(), Box::new(NoAgents)).unwrap();
    let mut state = worker.state.clone();
    for index in 0..1024 {
        state.create_room(&format!("room-{index}")).unwrap();
    }
    worker.save(state).unwrap();
    let (replies, answered) = mpsc::sync_channel(1024);
    let (commands, receiver) = mpsc::sync_channel(256);
    commands
        .send(call(
            "old-poll",
            "message.status",
            json!({"message":"1"}),
            None,
            &replies,
        ))
        .unwrap();
    // The actual interactive endpoint admits 16 callers at a time.
    for index in 0..16 {
        commands
            .send(call(
                &format!("state-{index}"),
                "state",
                json!({}),
                None,
                &replies,
            ))
            .unwrap();
    }
    let snapshot = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (events, _) = mpsc::channel();
    let thread = std::thread::spawn(move || worker.run(receiver, events, snapshot));
    let mut poll_seen_during_traffic = false;
    for index in 16..512 {
        let response = answered.recv_timeout(Duration::from_secs(10)).unwrap();
        poll_seen_during_traffic |= response.id == "old-poll";
        commands
            .send(call(
                &format!("state-{index}"),
                "state",
                json!({}),
                None,
                &replies,
            ))
            .unwrap();
    }
    commands.send((2, BusCommand::Shutdown)).unwrap();
    thread.join().unwrap();
    poll_seen_during_traffic |= answered
        .try_iter()
        .take_while(|reply| reply.id != "state-511")
        .any(|reply| reply.id == "old-poll");
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        poll_seen_during_traffic,
        "the oldest status poll ran only after all 512 later interactive reads stopped"
    );
}
