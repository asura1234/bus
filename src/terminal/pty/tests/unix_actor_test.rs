use super::*;
use std::{
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd, IntoRawFd},
    os::unix::net::UnixStream,
    sync::atomic::{AtomicBool, Ordering},
};

// The Windows actor's forwarder and writer are platform-independent threads;
// compiling them here runs its submission tests on Unix CI too.
#[allow(dead_code)]
mod windows_submission {
    include!("../actor/windows.rs");
}

fn test_wake_pair() -> (fd::WakeWriter, OwnedFd) {
    let pipe = fd::create_wake_pipe().expect("wake pipe");
    (pipe.writer, pipe.read_fd)
}

fn actor_with_socket_pair(
    initially_quiesced: bool,
) -> (PtyIoActorHandle, UnixStream, std_mpsc::Receiver<Bytes>) {
    actor_with_socket_pair_and_poll_observer(initially_quiesced, None)
}

fn actor_with_socket_pair_and_poll_observer(
    initially_quiesced: bool,
    poll_observer: Option<std_mpsc::Sender<()>>,
) -> (PtyIoActorHandle, UnixStream, std_mpsc::Receiver<Bytes>) {
    let (actor_socket, peer) = UnixStream::pair().expect("socket pair");
    actor_socket
        .set_nonblocking(true)
        .expect("actor socket nonblocking");
    peer.set_read_timeout(Some(Duration::from_secs(1)))
        .expect("peer timeout");
    let owned = unsafe { OwnedFd::from_raw_fd(actor_socket.into_raw_fd()) };
    let (read_tx, read_rx) = std_mpsc::channel();
    let config = PtyIoActorConfig {
        pane_id: 1,
        master_fd: owned,
        initially_quiesced,
        on_read: Box::new(move |bytes| {
            read_tx
                .send(Bytes::copy_from_slice(bytes))
                .expect("read callback receiver alive");
            PtyReadResult::empty()
        }),
    };
    let handle = if let Some(poll_observer) = poll_observer {
        PtyIoActor::spawn_with_poll_observer(config, poll_observer)
    } else {
        PtyIoActor::spawn(config)
    }
    .expect("actor spawn");
    (handle, peer, read_rx)
}

fn actor_runner_for_unit_test() -> (PtyIoActorRunner, UnixStream) {
    let (actor_socket, peer) = UnixStream::pair().expect("socket pair");
    actor_socket
        .set_nonblocking(true)
        .expect("actor socket nonblocking");
    let owned = unsafe { OwnedFd::from_raw_fd(actor_socket.into_raw_fd()) };
    let (_data_tx, data_rx) = mpsc::channel(ACTOR_COMMAND_BUFFER);
    let (_control_tx, control_rx) = std_mpsc::channel();
    let wake_pipe = fd::create_wake_pipe().expect("wake pipe");
    let runner = PtyIoActorRunner {
        pane_id: 1,
        file: std::fs::File::from(owned),
        data_rx,
        control_rx,
        state: ActorState::Running,
        pending_writes: VecDeque::new(),
        current_write_offset: 0,
        active_submission: None,
        wake_read_fd: wake_pipe.read_fd,
        controls: Arc::new(Mutex::new(SharedPtyControls::default())),
        response_order: Arc::new(Mutex::new(())),
        on_read: Box::new(|_| PtyReadResult::empty()),
        poll_observer: None,
    };
    (runner, peer)
}

#[test]
fn actor_ignores_empty_user_input_write() {
    let (mut runner, _peer) = actor_runner_for_unit_test();

    assert!(!runner.handle_data_command(PtyIoDataCommand::WriteUserInput(Bytes::new())));

    assert!(runner.pending_writes.is_empty());
}

#[test]
fn submission_boundary_does_not_wait_for_following_protocol_write() {
    let (mut runner, _peer) = actor_runner_for_unit_test();
    runner.enqueue_submission_write(Bytes::from_static(b"prompt"), SubmissionBoundary::Text);
    runner.enqueue_write(Bytes::from_static(b"response"));

    assert_eq!(
        runner.flush_pending_writes_once().unwrap(),
        Some(SubmissionBoundary::Text)
    );
    assert_eq!(
        runner.pending_writes[0].bytes,
        Bytes::from_static(b"response")
    );
}

#[test]
fn actor_writes_user_input_to_owned_fd() {
    let (handle, mut peer, _read_rx) = actor_with_socket_pair(false);

    handle
        .try_write_user_input(Bytes::from_static(b"hello"))
        .expect("write command accepted");

    let mut buf = [0u8; 5];
    peer.read_exact(&mut buf).expect("peer receives write");
    assert_eq!(&buf, b"hello");
    handle.shutdown();
}

#[test]
fn actor_delays_enter_from_completed_prompt_write() {
    let (handle, mut peer, _read_rx) = actor_with_socket_pair(false);
    let text = Bytes::from(vec![b'x'; 4 * 1024 * 1024]);
    let text_len = text.len();
    let delay = Duration::from_millis(200);
    let reader = std::thread::spawn(move || {
        std::thread::sleep(delay);
        let mut received = vec![0; text_len];
        peer.read_exact(&mut received)
            .expect("peer receives prompt");
        let prompt_completed = Instant::now();
        let mut enter = [0; 1];
        peer.read_exact(&mut enter).expect("peer receives enter");
        let enter_received = Instant::now();
        let mut user = [0; 4];
        peer.read_exact(&mut user)
            .expect("peer receives queued input");
        (prompt_completed, enter_received, enter, user)
    });

    let completion = handle
        .queue_user_input_submission(text, Bytes::from_static(b"\r"), delay)
        .expect("submission queues");
    handle
        .try_write_user_input(Bytes::from_static(b"user"))
        .expect("ordinary input queues behind submission");
    completion
        .recv()
        .expect("actor reports submission")
        .expect("submission completes");
    let (prompt_completed, enter_received, enter, user) = reader.join().expect("reader joins");

    assert_eq!(enter, *b"\r");
    assert_eq!(user, *b"user");
    assert!(enter_received.duration_since(prompt_completed) >= delay / 2);

    let err = match handle.queue_user_input_submission(
        Bytes::from_static(b"prompt"),
        Bytes::from_static(b"\r"),
        Duration::ZERO,
    ) {
        Ok(completion) => completion
            .recv()
            .expect("actor reports submission")
            .expect_err("closed PTY rejects submission"),
        Err(err) => err,
    };

    assert!(matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::WriteZero
    ));
}

#[test]
fn actor_completes_empty_submission_parts() {
    let (handle, mut peer, _read_rx) = actor_with_socket_pair(false);
    peer.set_read_timeout(Some(Duration::from_secs(1)))
        .expect("peer timeout");

    let completion = handle
        .queue_user_input_submission(Bytes::new(), Bytes::from_static(b"\r"), Duration::ZERO)
        .expect("empty prompt submission queues");
    let mut enter = [0; 1];
    peer.read_exact(&mut enter)
        .expect("peer receives enter for empty prompt");
    assert_eq!(enter, *b"\r");
    completion
        .recv_timeout(Duration::from_secs(1))
        .expect("actor reports empty prompt submission")
        .expect("empty prompt submission completes");

    let completion = handle
        .queue_user_input_submission(
            Bytes::from_static(b"prompt"),
            Bytes::new(),
            Duration::from_millis(40),
        )
        .expect("empty enter submission queues");
    let mut prompt = [0; 6];
    peer.read_exact(&mut prompt)
        .expect("peer receives prompt before empty enter");
    assert_eq!(&prompt, b"prompt");
    completion
        .recv_timeout(Duration::from_secs(1))
        .expect("actor reports empty enter submission")
        .expect("empty enter submission completes");
    handle.shutdown();
}

#[test]
fn actor_reports_peer_closure_during_submission_delay() {
    let (handle, mut peer, _read_rx) = actor_with_socket_pair(false);
    let completion = handle
        .queue_user_input_submission(
            Bytes::from_static(b"prompt"),
            Bytes::from_static(b"\r"),
            Duration::from_secs(1),
        )
        .expect("submission queues");
    let mut prompt = [0; 6];
    peer.read_exact(&mut prompt).expect("peer receives prompt");
    drop(peer);

    let err = completion
        .recv_timeout(Duration::from_secs(1))
        .expect("actor reports peer closure")
        .expect_err("peer closure fails the active submission");
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn actor_fails_buffered_submissions_on_exit() {
    let (handle, mut peer, _read_rx) = actor_with_socket_pair(false);
    let active = handle
        .queue_user_input_submission(
            Bytes::from_static(b"first"),
            Bytes::from_static(b"\r"),
            Duration::from_secs(1),
        )
        .expect("first submission queues");
    let mut prompt = [0; 5];
    peer.read_exact(&mut prompt).expect("peer receives prompt");
    let buffered = handle
        .queue_user_input_submission(
            Bytes::from_static(b"second"),
            Bytes::from_static(b"\r"),
            Duration::ZERO,
        )
        .expect("second submission queues");

    drop(peer);
    let active_err = active
        .recv_timeout(Duration::from_secs(1))
        .expect("actor reports active submission")
        .expect_err("peer closure fails active submission");
    let buffered_err = buffered
        .recv_timeout(Duration::from_secs(1))
        .expect("actor reports buffered submission")
        .expect_err("peer closure fails buffered submission");

    assert_eq!(active_err.kind(), std::io::ErrorKind::BrokenPipe);
    assert_eq!(buffered_err.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn actor_rejects_submission_after_io_loop_exits() {
    let (mut runner, peer) = actor_runner_for_unit_test();
    let (data_tx, data_rx) = mpsc::channel(ACTOR_COMMAND_BUFFER);
    let (control_tx, control_rx) = std_mpsc::channel();
    let (wake, wake_read_fd) = test_wake_pair();
    runner.data_rx = data_rx;
    runner.control_rx = control_rx;
    runner.wake_read_fd = wake_read_fd;
    let handle = PtyIoActorHandle {
        data_tx,
        control_tx,
        wake,
        user_writes: Arc::new(Mutex::new(UserWriteGate { accepting: true })),
        controls: Arc::clone(&runner.controls),
        response_order: Arc::clone(&runner.response_order),
    };
    let (exit_tx, exit_rx) = std_mpsc::channel();
    let actor = std::thread::spawn(move || {
        runner.run();
        exit_tx.send(()).expect("loop exit receiver alive");
        runner
    });

    drop(peer);
    exit_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("actor io loop exited");
    // Keep the receiver alive so this checks explicit closure, not the runner's drop.
    let _runner = actor.join().expect("actor thread joins");
    let err = handle
        .queue_user_input_submission(
            Bytes::from_static(b"prompt"),
            Bytes::from_static(b"\r"),
            Duration::ZERO,
        )
        .expect_err("closed actor rejects submission");

    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn actor_wakes_idle_poll_for_user_input() {
    let (poll_tx, poll_rx) = std_mpsc::channel();
    let (handle, mut peer, _read_rx) =
        actor_with_socket_pair_and_poll_observer(false, Some(poll_tx));
    peer.set_read_timeout(Some(Duration::from_millis(500)))
        .expect("peer timeout");
    poll_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("actor entered idle poll");

    let start = Instant::now();
    handle
        .try_write_user_input(Bytes::from_static(b"x"))
        .expect("write command accepted");

    let mut buf = [0u8; 1];
    peer.read_exact(&mut buf)
        .expect("peer receives write without waiting for actor poll timeout");
    assert_eq!(&buf, b"x");
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "actor write should be driven by wake fd, not the idle poll timeout"
    );
    handle.shutdown();
}

#[test]
fn actor_reads_output_while_input_is_backpressured() {
    let (mut actor_socket, mut peer) = UnixStream::pair().expect("socket pair");
    actor_socket
        .set_nonblocking(true)
        .expect("actor socket nonblocking");
    peer.set_read_timeout(Some(Duration::from_secs(1)))
        .expect("peer timeout");

    let fill = [0xAA; 8192];
    let mut prefilled = 0;
    loop {
        match actor_socket.write(&fill) {
            Ok(written) => prefilled += written,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(err) => panic!("failed to fill actor write buffer: {err}"),
        }
    }
    assert!(prefilled > 0, "actor write buffer should accept some bytes");

    let owned = unsafe { OwnedFd::from_raw_fd(actor_socket.into_raw_fd()) };
    let (read_tx, read_rx) = std_mpsc::channel();
    let handle = PtyIoActor::spawn(PtyIoActorConfig {
        pane_id: 1,
        master_fd: owned,
        initially_quiesced: false,
        on_read: Box::new(move |bytes| {
            read_tx
                .send(Bytes::copy_from_slice(bytes))
                .expect("read callback receiver alive");
            PtyReadResult::empty()
        }),
    })
    .expect("actor spawn");

    let marker = Bytes::from_static(b"queued-input");
    let completion = handle
        .queue_user_input_submission(marker.clone(), Bytes::from_static(b"\r"), Duration::ZERO)
        .expect("submission accepted");

    const OUTPUT_LEN: usize = 128 * 1024;
    let mut peer_writer = peer.try_clone().expect("clone peer writer");
    let output_writer = std::thread::spawn(move || {
        peer_writer
            .write_all(&vec![0xBB; OUTPUT_LEN])
            .expect("peer writes sustained output");
    });
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut output_len = 0;
    while output_len < OUTPUT_LEN {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "actor did not keep reading blocked peer output"
        );
        let output = read_rx
            .recv_timeout(remaining)
            .expect("actor keeps reading while input remains blocked");
        assert!(output.iter().all(|byte| *byte == 0xBB));
        output_len += output.len();
    }
    assert_eq!(output_len, OUTPUT_LEN);
    output_writer.join().expect("output writer joins");

    let mut received_input = vec![0; prefilled + marker.len() + 1];
    peer.read_exact(&mut received_input)
        .expect("peer receives prefill and queued input");
    assert!(received_input[..prefilled].iter().all(|byte| *byte == 0xAA));
    assert_eq!(
        &received_input[prefilled..prefilled + marker.len()],
        marker.as_ref()
    );
    assert_eq!(received_input.last(), Some(&b'\r'));
    completion
        .recv_timeout(Duration::from_secs(1))
        .expect("actor reports submission")
        .expect("submission completes");
    handle.shutdown();
}

#[test]
fn poll_ignores_pty_hup_without_pty_interest() {
    let (actor_socket, peer) = UnixStream::pair().expect("socket pair");
    actor_socket
        .set_nonblocking(true)
        .expect("actor socket nonblocking");
    drop(peer);
    let wake_pipe = fd::create_wake_pipe().expect("wake pipe");

    let readiness = fd::poll_pty_and_wake(
        actor_socket.as_raw_fd(),
        wake_pipe.read_fd.as_raw_fd(),
        false,
        false,
        10,
    )
    .expect("poll succeeds");

    assert!(!readiness.pty_read_ready);
    assert!(!readiness.pty_write_ready);
    assert!(!readiness.wake_ready);
}

#[test]
fn actor_delivers_fd_reads_to_callback() {
    let (handle, mut peer, read_rx) = actor_with_socket_pair(false);

    peer.write_all(b"from-peer").expect("peer write");

    let read = read_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("actor read callback");
    assert_eq!(read, Bytes::from_static(b"from-peer"));
    handle.shutdown();
}

#[test]
fn resize_keeps_latest_request_when_command_queue_is_full() {
    let (data_tx, _data_rx) = mpsc::channel(1);
    let (control_tx, _control_rx) = std_mpsc::channel();
    data_tx
        .try_send(PtyIoDataCommand::WriteUserInput(Bytes::from_static(
            b"fill",
        )))
        .expect("fill command queue");
    let controls = Arc::new(Mutex::new(SharedPtyControls::default()));
    let (wake, _wake_read_fd) = test_wake_pair();
    let handle = PtyIoActorHandle {
        data_tx,
        control_tx,
        wake,
        user_writes: Arc::new(Mutex::new(UserWriteGate { accepting: true })),
        controls: Arc::clone(&controls),
        response_order: Arc::new(Mutex::new(())),
    };

    handle.resize(20, 80, 8, 16, vec![Bytes::from_static(b"old")]);
    handle.resize(40, 120, 9, 18, vec![Bytes::from_static(b"new")]);
    handle.write_terminal_response(|| Some(Bytes::from_static(b"response")));

    let controls = controls.lock().expect("controls lock");
    assert_eq!(
        controls.resize,
        Some(PtyResizeRequest {
            resize: PtyResize {
                rows: 40,
                cols: 120,
                cell_width_px: 9,
                cell_height_px: 18,
            },
            terminal_responses: vec![Bytes::from_static(b"new")],
        })
    );
    assert_eq!(
        controls.terminal_responses,
        vec![Bytes::from_static(b"response")]
    );
}

#[test]
fn appearance_transition_report_precedes_query_of_new_scheme() {
    let (actor_socket, mut peer) = UnixStream::pair().expect("socket pair");
    actor_socket
        .set_nonblocking(true)
        .expect("actor socket nonblocking");
    let owned = unsafe { OwnedFd::from_raw_fd(actor_socket.into_raw_fd()) };
    let (data_tx, data_rx) = mpsc::channel(ACTOR_COMMAND_BUFFER);
    let (control_tx, control_rx) = std_mpsc::channel();
    let wake_pipe = fd::create_wake_pipe().expect("wake pipe");
    let controls = Arc::new(Mutex::new(SharedPtyControls::default()));
    let response_order = Arc::new(Mutex::new(()));
    let light = Arc::new(AtomicBool::new(false));
    let query_light = Arc::clone(&light);
    let runner = PtyIoActorRunner {
        pane_id: 1,
        file: std::fs::File::from(owned),
        data_rx,
        control_rx,
        state: ActorState::Running,
        pending_writes: VecDeque::new(),
        current_write_offset: 0,
        active_submission: None,
        wake_read_fd: wake_pipe.read_fd,
        controls: Arc::clone(&controls),
        response_order: Arc::clone(&response_order),
        on_read: Box::new(move |_| PtyReadResult {
            terminal_responses: vec![if query_light.load(Ordering::Acquire) {
                Bytes::from_static(b"query-light")
            } else {
                Bytes::from_static(b"query-dark")
            }],
        }),
        poll_observer: None,
    };
    let handle = PtyIoActorHandle {
        data_tx,
        control_tx,
        wake: wake_pipe.writer,
        user_writes: Arc::new(Mutex::new(UserWriteGate { accepting: true })),
        controls,
        response_order,
    };
    let (changed_tx, changed_rx) = std_mpsc::channel();
    let (continue_tx, continue_rx) = std_mpsc::channel();

    let appearance = std::thread::spawn(move || {
        handle.write_terminal_response(|| {
            light.store(true, Ordering::Release);
            changed_tx.send(()).expect("notify appearance change");
            continue_rx.recv().expect("continue appearance report");
            Some(Bytes::from_static(b"live-light"))
        });
    });
    changed_rx.recv().expect("appearance changed");
    peer.write_all(b"query").expect("write query");
    let reader = std::thread::spawn(move || {
        let mut runner = runner;
        assert!(runner.read_once());
        runner
    });
    continue_tx.send(()).expect("release appearance report");
    appearance.join().expect("appearance thread joins");
    let runner = reader.join().expect("reader thread joins");

    assert_eq!(
        runner.pending_writes,
        VecDeque::from([
            PendingWrite {
                bytes: Bytes::from_static(b"live-light"),
                boundary: None,
            },
            PendingWrite {
                bytes: Bytes::from_static(b"query-light"),
                boundary: None,
            },
        ])
    );
}

#[test]
fn resize_writes_terminal_responses_after_applying_resize() {
    let (handle, mut peer, _read_rx) = actor_with_socket_pair(false);
    let response = Bytes::from_static(b"\x1B[48;40;100;720;900t");

    handle.resize(40, 100, 9, 18, vec![response.clone()]);

    let mut buf = vec![0; response.len()];
    peer.read_exact(&mut buf)
        .expect("peer receives resize response");
    assert_eq!(Bytes::from(buf), response);
    handle.shutdown();
}

#[test]
fn queued_input_runs_promptly_after_text_only_submission_delay() {
    let (poll_tx, poll_rx) = std_mpsc::channel();
    let (handle, mut peer, _read_rx) =
        actor_with_socket_pair_and_poll_observer(false, Some(poll_tx));
    peer.set_read_timeout(Some(Duration::from_millis(500)))
        .expect("peer timeout");
    let completion = handle
        .queue_user_input_submission(
            Bytes::from_static(b"prompt"),
            Bytes::new(),
            Duration::from_millis(150),
        )
        .expect("text-only submission accepted");
    let mut prompt = [0; 6];
    peer.read_exact(&mut prompt).expect("prompt reaches peer");
    assert_eq!(&prompt, b"prompt");
    while poll_rx.try_recv().is_ok() {}

    handle
        .try_write_user_input(Bytes::from_static(b"user"))
        .expect("input queues during submission delay");
    poll_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("actor consumes wake for queued input");
    completion
        .recv_timeout(Duration::from_secs(1))
        .expect("text-only submission reports completion")
        .expect("text-only submission succeeds");

    let mut user = [0; 4];
    let received = peer.read_exact(&mut user);
    handle.shutdown();
    received.expect("queued input must not wait for the one-second idle poll after completion");
    assert_eq!(&user, b"user");
}
