// Deliberately fill only this actor's socket: no scheduler or machine-load fault.
fn backpressured_actor() -> (PtyIoActorHandle, UnixStream, usize) {
    let (mut socket, peer) = UnixStream::pair().unwrap();
    socket.set_nonblocking(true).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut prefilled = 0;
    for fill in [&[0xAA; 8192][..], &[0xAA][..]] {
        loop {
            match socket.write(fill) {
                Ok(n) => prefilled += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                other => panic!("backpressure setup: {other:?}"),
            }
        }
    }
    assert!(prefilled > 0);
    let actor = PtyIoActor::spawn(PtyIoActorConfig {
        pane_id: 1,
        master_fd: socket.into(),
        initially_quiesced: false,
        on_read: Box::new(|_| PtyReadResult::empty()),
    })
    .unwrap();
    (actor, peer, prefilled)
}

fn assert_only_following_input(actor: &PtyIoActorHandle, peer: &mut UnixStream, prefilled: usize) {
    actor
        .try_write_user_input(Bytes::from_static(b"after"))
        .unwrap();
    let mut received = vec![0; prefilled + 5];
    let read = peer.read_exact(&mut received);
    actor.shutdown();
    read.unwrap();
    assert!(received[..prefilled].iter().all(|b| *b == 0xAA));
    // This marker proves the actor resumed, serviced subsequent input and did
    // not leave the cancelled key to be written later.
    assert_eq!(&received[prefilled..], b"after");
}

#[test]
fn waiter_timeout_cancels_backpressured_input_before_its_write_deadline() {
    let (actor, mut peer, prefilled) = backpressured_actor();
    let completion = actor
        .queue_user_input_submission(
            Bytes::from_static(b"\x01"),
            Bytes::from_static(b"\r"),
            Duration::ZERO,
            Some(Instant::now() + Duration::from_secs(60)),
        )
        .unwrap();
    assert!(matches!(
        completion.recv_timeout(Duration::ZERO),
        Err(std_mpsc::RecvTimeoutError::Timeout)
    ));
    // Keep the receiver alive: timeout itself must cancel, without relying on Drop.
    assert_only_following_input(&actor, &mut peer, prefilled);
    drop(completion);
}

#[test]
fn dropped_waiter_cancels_backpressured_input() {
    let (actor, mut peer, prefilled) = backpressured_actor();
    let completion = actor
        .queue_user_input_submission(
            Bytes::from_static(b"\x01"),
            Bytes::from_static(b"\r"),
            Duration::ZERO,
            None,
        )
        .unwrap();
    drop(completion);
    assert_only_following_input(&actor, &mut peer, prefilled);
}

#[test]
fn expired_command_is_dropped_when_dequeued_behind_another_submission() {
    let (actor, mut peer, prefilled) = backpressured_actor();
    let blocking = actor
        .queue_user_input_submission(
            Bytes::from_static(b"blocked"),
            Bytes::new(),
            Duration::ZERO,
            None,
        )
        .unwrap();
    let expired = actor
        .queue_user_input_submission(
            Bytes::from_static(b"expired"),
            Bytes::from_static(b"\r"),
            Duration::ZERO,
            Some(Instant::now() - Duration::from_secs(1)),
        )
        .unwrap();
    drop(blocking);
    let error = expired
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_only_following_input(&actor, &mut peer, prefilled);
}

#[test]
fn legacy_submission_never_leaves_text_without_enter() {
    // An unobserved prompt has no withdrawal step: if its Enter cannot fit
    // before the deadline, its text must not be written either.
    let (actor, mut peer, _) = actor_with_socket_pair(false);
    let result = actor
        .queue_user_input_submission(
            Bytes::from_static(b"prompt"),
            Bytes::from_static(b"\r"),
            Duration::from_millis(200),
            Some(Instant::now() + Duration::from_millis(100)),
        )
        .unwrap()
        .into_receiver()
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    actor
        .try_write_user_input(Bytes::from_static(b"after"))
        .unwrap();
    let mut received = Vec::new();
    while !received.ends_with(b"after") {
        let mut chunk = [0; 64];
        let n = peer.read(&mut chunk).unwrap();
        received.extend_from_slice(&chunk[..n]);
    }
    actor.shutdown();
    let expected: &[u8] = if result.is_ok() {
        b"prompt\rafter"
    } else {
        b"after"
    };
    assert_eq!(
        String::from_utf8_lossy(&received),
        String::from_utf8_lossy(expected),
        "submission result {result:?} left a partial prompt"
    );
}

#[test]
fn cancelling_partial_text_preserves_protocol_writes_and_resets_the_offset() {
    let (mut runner, mut peer) = actor_runner_for_unit_test();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let (wake, _wake_reader) = test_wake_pair();
    let permit = WritePermit::new(None, wake);
    let (reply, receiver) = std_mpsc::channel();
    let completion = InputCompletion::tracked(receiver, permit.clone());
    let text_len = 4 * 1024 * 1024;
    runner.handle_data_command(PtyIoDataCommand::SubmitUserInput {
        text: Bytes::from(vec![b'x'; text_len]),
        enter: Bytes::from_static(b"\r"),
        delay: Duration::ZERO,
        permit,
        reply,
    });
    runner.enqueue_write(Bytes::from_static(b"response"));
    assert_eq!(runner.flush_pending_writes_once().unwrap(), None);
    let written = runner.current_write_offset;
    assert!(
        written > 0 && written < text_len,
        "must exercise partial write and WouldBlock"
    );
    assert!(completion.recv_timeout(Duration::ZERO).is_err());
    runner.expire_submission();
    assert!(runner.active_submission.is_none());
    assert_eq!(runner.current_write_offset, 0);
    let mut prefix = vec![0; written];
    peer.read_exact(&mut prefix).unwrap();
    assert!(prefix.iter().all(|b| *b == b'x'));
    runner.enqueue_write(Bytes::from_static(b"after"));
    assert_eq!(runner.flush_pending_writes_once().unwrap(), None);
    let mut subsequent = [0; 13];
    peer.read_exact(&mut subsequent).unwrap();
    assert_eq!(&subsequent, b"responseafter");
}

#[test]
fn backpressured_legacy_text_expires_with_time_reserved_for_enter_delay() {
    let (actor, mut peer, prefilled) = backpressured_actor();
    let completion = actor
        .queue_user_input_submission(
            Bytes::from_static(b"prompt"),
            Bytes::from_static(b"\r"),
            Duration::from_millis(59_900),
            Some(Instant::now() + Duration::from_secs(60)),
        )
        .unwrap()
        .into_receiver();
    let result = completion.recv_timeout(Duration::from_secs(2));
    if result.is_err() {
        actor.shutdown();
    }
    let error = result
        .expect("text must expire before consuming the Enter delay reserve")
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_only_following_input(&actor, &mut peer, prefilled);
}

#[test]
fn committed_legacy_submission_finishes_after_backpressure_passes_deadline() {
    let (actor, mut peer, _) = actor_with_socket_pair(false);
    let text_len = 4 * 1024 * 1024;
    let deadline = Instant::now() + Duration::from_millis(300);
    let completion = actor
        .queue_user_input_submission(
            Bytes::from(vec![b'x'; text_len]),
            Bytes::from_static(b"\r"),
            Duration::from_millis(50),
            Some(deadline),
        )
        .unwrap()
        .into_receiver();
    // Observe a real first byte before holding local pressure across expiry.
    let mut first = [0; 1];
    peer.read_exact(&mut first).unwrap();
    assert_eq!(first, [b'x']);
    actor
        .try_write_user_input(Bytes::from_static(b"after"))
        .unwrap();
    let reader = std::thread::spawn(move || {
        std::thread::sleep(
            deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(20),
        );
        let mut received = vec![0; text_len + 1 + 5];
        received[0] = first[0];
        let read = peer.read_exact(&mut received[1..]);
        (read, received)
    });
    let result = completion.recv_timeout(Duration::from_secs(3));
    if !result.as_ref().is_ok_and(|result| result.is_ok()) {
        actor.shutdown();
    }
    let (read, received) = reader.join().unwrap();
    actor.shutdown();
    result
        .expect("committed text and Enter must finish")
        .expect("must not discard a committed Enter");
    read.unwrap();
    assert!(received[..text_len].iter().all(|byte| *byte == b'x'));
    assert_eq!(&received[text_len..], b"\rafter");
}

#[test]
fn write_deadline_expires_during_enter_only_delay() {
    let (actor, mut peer, _) = actor_with_socket_pair(false);
    let completion = actor
        .queue_user_input_submission(
            Bytes::new(),
            Bytes::from_static(b"\r"),
            Duration::from_secs(60),
            Some(Instant::now() + Duration::from_millis(100)),
        )
        .unwrap();
    let error = completion
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    actor
        .try_write_user_input(Bytes::from_static(b"after"))
        .unwrap();
    let mut after = [0; 5];
    let read = peer.read_exact(&mut after);
    actor.shutdown();
    read.unwrap();
    assert_eq!(&after, b"after");
}
