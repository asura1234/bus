use super::*;

#[test]
fn client_writer_queue_keeps_render_slot_bounded() {
    let (writer, _queue) = test_queue_writer();
    let first = frame_server_message(&ServerMessage::WindowTitle {
        title: Some("first".into()),
    });
    let second = frame_server_message(&ServerMessage::WindowTitle {
        title: Some("second".into()),
    });

    writer.render.try_send(first).expect("first render fits");
    assert!(matches!(
        writer.render.try_send(second),
        Err(TrySendError::Full(_))
    ));
}

#[test]
fn client_writer_prioritizes_control_and_reports_render_drain() {
    let (mut client_stream, server_stream, _path) = local_stream_pair("client-writer-priority");
    let (writer, queue) = test_queue_writer();
    writer
        .render
        .try_send(frame_server_message(&ServerMessage::WindowTitle {
            title: Some("render".into()),
        }))
        .expect("queue render");
    writer
        .control
        .send(frame_server_message(&ServerMessage::ReloadSoundConfig))
        .expect("queue control");

    let (server_event_tx, mut server_event_rx) = mpsc::channel(4);
    let handle = std::thread::spawn(move || {
        client_writer_loop(server_stream, 9, queue, server_event_tx);
    });

    match protocol::read_message(&mut client_stream, MAX_FRAME_SIZE).expect("read control") {
        ServerMessage::ReloadSoundConfig => {}
        other => panic!("expected control message first, got {other:?}"),
    }
    match protocol::read_message(&mut client_stream, MAX_FRAME_SIZE).expect("read render") {
        ServerMessage::WindowTitle { title } => assert_eq!(title.as_deref(), Some("render")),
        other => panic!("expected render message second, got {other:?}"),
    }
    match server_event_rx
        .blocking_recv()
        .expect("writer drained render slot")
    {
        ServerEvent::ClientWriterDrained { client_id } => assert_eq!(client_id, 9),
        other => panic!("expected writer drained event, got {other:?}"),
    }

    drop(writer);
    handle.join().expect("writer exits after senders drop");
}

#[test]
fn client_writer_exits_when_all_writer_handles_drop() {
    let (_client_stream, server_stream, _path) = local_stream_pair("client-writer-drop");
    let (writer, queue) = test_queue_writer();
    let (server_event_tx, _server_event_rx) = mpsc::channel(4);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        client_writer_loop(server_stream, 11, queue, server_event_tx);
        let _ = done_tx.send(());
    });

    drop(writer);
    done_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("writer exits without polling after senders drop");
}

#[test]
fn client_writer_clone_keeps_loop_alive_until_final_drop() {
    let (mut client_stream, server_stream, _path) = local_stream_pair("client-writer-clone-drop");
    let (writer, queue) = test_queue_writer();
    let cloned_writer = writer.clone();
    let (server_event_tx, _server_event_rx) = mpsc::channel(4);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        client_writer_loop(server_stream, 12, queue, server_event_tx);
        let _ = done_tx.send(());
    });

    drop(writer);
    cloned_writer
        .control
        .send(frame_server_message(&ServerMessage::ReloadSoundConfig))
        .expect("cloned writer still sends after original drops");
    match protocol::read_message(&mut client_stream, MAX_FRAME_SIZE)
        .expect("read control from cloned writer")
    {
        ServerMessage::ReloadSoundConfig => {}
        other => panic!("expected cloned control message, got {other:?}"),
    }
    assert!(
        done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "writer exited while cloned handles were still alive"
    );

    drop(cloned_writer);
    done_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("writer exits after final cloned writer drops");
}

#[test]
fn client_writer_closes_queue_after_socket_write_failure() {
    let (client_stream, server_stream, _path) = local_stream_pair("client-writer-socket-failure");
    #[cfg(not(windows))]
    server_stream
        .set_send_timeout(Some(Duration::from_millis(100)))
        .expect("set test send timeout");
    let (writer, queue) = test_queue_writer();
    let (server_event_tx, _server_event_rx) = mpsc::channel(4);
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        client_writer_loop(server_stream, 13, queue, server_event_tx);
        let _ = done_tx.send(());
    });

    drop(client_stream);
    writer
        .control
        .send(vec![b'x'; 1024 * 1024])
        .expect("message is accepted before the writer observes socket failure");
    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer exits after socket write failure");

    assert!(matches!(writer.control.send(vec![b'y']), Err(SendError(_))));
    assert!(matches!(
        writer.render.try_send(vec![b'z']),
        Err(TrySendError::Disconnected(_))
    ));
}
