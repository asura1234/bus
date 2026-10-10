use super::*;
struct RecordingWriter {
    writes: Vec<(Vec<u8>, Instant)>,
    flushes: Vec<Instant>,
    fail_after: Option<usize>,
    flushed: std_mpsc::Sender<()>,
}

impl Write for RecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.fail_after == Some(self.writes.len()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "writer closed",
            ));
        }
        self.writes.push((bytes.to_vec(), Instant::now()));
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flushes.push(Instant::now());
        let _ = self.flushed.send(());
        Ok(())
    }
}

fn run_recorded_submission(
    fail_after: Option<usize>,
    delay: Duration,
    deadline: Option<Instant>,
    during_delay: impl FnOnce(&std_mpsc::Sender<PtyIoWriteCommand>, &Arc<Mutex<bool>>),
) -> (RecordingWriter, std::io::Result<()>) {
    let (flushed_tx, flushed_rx) = std_mpsc::channel();
    let mut writer = RecordingWriter {
        writes: Vec::new(),
        flushes: Vec::new(),
        fail_after,
        flushed: flushed_tx,
    };
    let (data_tx, mut data_rx) = mpsc::channel(2);
    let (write_tx, write_rx) = std_mpsc::channel();
    let (reply_tx, reply_rx) = std_mpsc::channel();
    let accepting = Arc::new(Mutex::new(true));
    data_tx
        .try_send(PtyIoDataCommand::SubmitUserInput {
            text: Bytes::from_static(b"prompt"),
            enter: Bytes::from_static(b"\r"),
            delay,
            deadline,
            reply: reply_tx,
        })
        .unwrap();
    data_tx
        .try_send(PtyIoDataCommand::WriteUserInput(Bytes::from_static(
            b"user",
        )))
        .unwrap();
    let writer_thread = std::thread::spawn(move || {
        run_writer(&mut writer, write_rx);
        writer
    });
    let input_write_tx = write_tx.clone();
    let input_accepting = Arc::clone(&accepting);
    let input_thread = std::thread::spawn(move || {
        run_input_forwarder(&mut data_rx, input_write_tx, input_accepting)
    });
    flushed_rx.recv().expect("prompt was flushed");
    during_delay(&write_tx, &accepting);
    let result = reply_rx.recv().expect("writer reports submission");
    drop(data_tx);
    input_thread.join().expect("input thread joins");
    drop(write_tx);
    (writer_thread.join().expect("writer thread joins"), result)
}

#[test]
fn submission_sequences_user_input_but_allows_terminal_responses() {
    let delay = Duration::from_millis(30);
    let (writer, result) = run_recorded_submission(None, delay, None, |write_tx, _| {
        write_tx
            .send(PtyIoWriteCommand::Write(Bytes::from_static(b"response")))
            .unwrap();
    });
    result.expect("submission succeeds");

    assert_eq!(writer.writes[0].0, b"prompt");
    assert_eq!(writer.writes[1].0, b"response");
    assert_eq!(writer.writes[2].0, b"\r");
    assert_eq!(writer.writes[3].0, b"user");
    assert!(writer.writes[2].1.duration_since(writer.flushes[0]) >= delay);
}

#[test]
fn submission_returns_enter_write_failure() {
    let (_writer, result) = run_recorded_submission(Some(1), Duration::ZERO, None, |_, _| {});
    let err = result.expect_err("enter failure reaches caller");

    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn shutdown_during_submission_delay_cancels_enter() {
    let (writer, result) =
        run_recorded_submission(None, Duration::from_millis(30), None, |_, accepting| {
            *accepting.lock().unwrap() = false;
        });
    let err = result.expect_err("shutdown cancels enter");
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    assert_eq!(
        writer
            .writes
            .iter()
            .map(|write| write.0.as_slice())
            .collect::<Vec<_>>(),
        vec![b"prompt"]
    );
}

#[test]
fn expired_queued_submission_is_not_written() {
    let (flushed_tx, _flushed_rx) = std_mpsc::channel();
    let mut writer = RecordingWriter {
        writes: Vec::new(),
        flushes: Vec::new(),
        fail_after: None,
        flushed: flushed_tx,
    };
    let (data_tx, mut data_rx) = mpsc::channel(2);
    let (write_tx, write_rx) = std_mpsc::channel();
    let (first_reply_tx, first_reply_rx) = std_mpsc::channel();
    let (expired_reply_tx, expired_reply_rx) = std_mpsc::channel();
    let accepting = Arc::new(Mutex::new(true));
    data_tx
        .try_send(PtyIoDataCommand::SubmitUserInput {
            text: Bytes::from_static(b"first"),
            enter: Bytes::from_static(b"\r"),
            delay: Duration::from_millis(30),
            deadline: None,
            reply: first_reply_tx,
        })
        .unwrap();
    data_tx
        .try_send(PtyIoDataCommand::SubmitUserInput {
            text: Bytes::from_static(b"expired"),
            enter: Bytes::from_static(b"\r"),
            delay: Duration::ZERO,
            deadline: Some(Instant::now() + Duration::from_millis(10)),
            reply: expired_reply_tx,
        })
        .unwrap();

    let writer_thread = std::thread::spawn(move || {
        run_writer(&mut writer, write_rx);
        writer
    });
    let input_write_tx = write_tx.clone();
    let input_thread =
        std::thread::spawn(move || run_input_forwarder(&mut data_rx, input_write_tx, accepting));
    first_reply_rx.recv().unwrap().unwrap();
    let err = expired_reply_rx.recv().unwrap().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);

    drop(data_tx);
    input_thread.join().unwrap();
    drop(write_tx);
    let writer = writer_thread.join().unwrap();
    assert_eq!(
        writer
            .writes
            .iter()
            .map(|write| write.0.as_slice())
            .collect::<Vec<_>>(),
        vec![b"first".as_slice(), b"\r".as_slice()]
    );
}

#[test]
fn shutdown_returns_while_submission_enter_is_backpressured() {
    let (data_tx, mut data_rx) = mpsc::channel(1);
    let (control_tx, _control_rx) = std_mpsc::channel();
    let (write_tx, write_rx) = std_mpsc::channel();
    let accepting = Arc::new(Mutex::new(true));
    let handle = PtyIoActorHandle {
        data_tx,
        control_tx,
        write_tx: write_tx.clone(),
        response_order: Arc::new(Mutex::new(())),
        accepting: Arc::clone(&accepting),
    };
    let completion = handle
        .queue_user_input_submission(
            Bytes::from_static(b"prompt"),
            Bytes::from_static(b"\r"),
            Duration::ZERO,
            None,
        )
        .expect("submission queues");
    let input_thread = std::thread::spawn(move || {
        run_input_forwarder(&mut data_rx, write_tx, accepting);
    });

    let PtyIoWriteCommand::SubmissionPart { bytes, reply, .. } = write_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("text write queued")
    else {
        panic!("expected text submission part");
    };
    assert_eq!(bytes.as_ref(), b"prompt");
    reply.send(Ok(())).expect("text flush completes");
    let PtyIoWriteCommand::SubmissionPart { bytes, reply, .. } = write_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("enter write queued")
    else {
        panic!("expected Enter submission part");
    };
    assert_eq!(bytes.as_ref(), b"\r");

    let (shutdown_tx, shutdown_rx) = std_mpsc::channel();
    let shutdown_thread = std::thread::spawn(move || {
        handle.shutdown();
        shutdown_tx.send(()).expect("shutdown observer alive");
    });
    let shutdown_result = shutdown_rx.recv_timeout(Duration::from_millis(250));

    // Release the pending write before asserting so a failed probe cannot
    // leave either worker blocked after the test returns.
    reply.send(Ok(())).expect("release Enter write");
    completion
        .recv_timeout(Duration::from_secs(1))
        .expect("submission reports completion")
        .expect("submission succeeds once Enter is flushed");
    shutdown_thread.join().expect("shutdown thread joins");
    input_thread.join().expect("input thread joins");
    shutdown_result.expect("shutdown must return before a blocked Enter write completes");
}
