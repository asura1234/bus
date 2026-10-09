use super::*;

#[test]
fn framing_small_message_roundtrip() {
    let msg = ClientMessage::TerminalHello {
        version: PROTOCOL_VERSION,
        cols: 80,
        rows: 24,
        cell_width_px: 8,
        cell_height_px: 16,
        pixel_mouse: false,
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &msg).unwrap();
    let decoded: ClientMessage = read_message(&mut buf.as_slice(), MAX_FRAME_SIZE).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn framing_large_payload_roundtrip() {
    // Create a pane-surface message that is ≥128 KB.
    // Use a large frame with verbose cell data to exceed 128 KB after bincode encoding.
    // 200×50 = 10000 cells. With varied symbols and styles, this should easily exceed 128 KB.
    let width: u16 = 200;
    let height: u16 = 50;
    let cells: Vec<CellData> = (0..(width as usize) * (height as usize))
        .map(|i| CellData {
            symbol: if i % 256 < 32 {
                " ".to_owned()
            } else {
                format!("{:03}", i % 1000)
            },
            fg: color_to_u32(Color::Rgb((i % 256) as u8, ((i / 256) % 256) as u8, 128)),
            bg: color_to_u32(Color::Indexed((i % 256) as u8)),
            modifier: ((i % 16) as u16),
            skip: i % 100 == 0,
            hyperlink: None,
        })
        .collect();

    let frame = FrameData {
        cells,
        width,
        height,
        cursor: Some(CursorState {
            x: 10,
            y: 5,
            visible: true,
            shape: 0,
        }),
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let msg = ServerMessage::PaneSurface(PaneSurfaceFrame {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        surface_revision: 1,
        frame,
        panes: Vec::new(),
        splits: Vec::new(),
        graphics: SurfaceGraphicsScene::default(),
    });

    let mut buf = Vec::new();
    write_message(&mut buf, &msg).unwrap();
    // Verify the payload is at least 128 KB
    assert!(
        buf.len() >= 128 * 1024,
        "framed payload should be >= 128 KB, got {} bytes",
        buf.len()
    );

    let decoded: ServerMessage = read_message(&mut buf.as_slice(), MAX_FRAME_SIZE).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn framing_multiple_messages_sequential() {
    // Write 100+ messages of varying types and read them back.
    let mut buf = Vec::new();
    let mut expected = Vec::new();

    for i in 0..150u32 {
        let msg = match i % 5 {
            0 => ClientMessage::TerminalHello {
                version: PROTOCOL_VERSION,
                cols: (80 + (i % 40) as u16),
                rows: (24 + (i % 20) as u16),
                cell_width_px: 8,
                cell_height_px: 16,
                pixel_mouse: i % 2 == 0,
            },
            1 => ClientMessage::Input {
                data: vec![(i % 256) as u8; (i as usize % 50) + 1],
            },
            2 => ClientMessage::ClipboardImage {
                target: ClientClipboardImageTarget::Pane("w1:p1".into()),
                extension: "png".to_owned(),
                data: vec![0x89, b'P', b'N', b'G', (i % 256) as u8],
            },
            3 => ClientMessage::Resize {
                cols: (100 + (i % 30) as u16),
                rows: (30 + (i % 10) as u16),
                cell_width_px: 8,
                cell_height_px: 16,
                pixel_mouse: i % 2 == 0,
            },
            4 => ClientMessage::Detach,
            _ => unreachable!(),
        };
        write_message(&mut buf, &msg).unwrap();
        expected.push(msg);
    }

    let mut cursor = buf.as_slice();
    for expected_msg in &expected {
        let decoded: ClientMessage = read_message(&mut cursor, MAX_FRAME_SIZE).unwrap();
        assert_eq!(*expected_msg, decoded);
    }
}

#[test]
fn framing_oversized_rejected_without_panic() {
    // Craft a frame with a huge length prefix (4 GB claim).
    let mut buf: Vec<u8> = (u32::MAX).to_le_bytes().to_vec();
    // Add a few garbage bytes after the length prefix.
    buf.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);

    let result: Result<ClientMessage, FramingError> =
        read_message(&mut buf.as_slice(), MAX_FRAME_SIZE);
    match result {
        Err(FramingError::Oversized { claimed, max }) => {
            assert_eq!(claimed, u32::MAX as usize);
            assert_eq!(max, MAX_FRAME_SIZE);
        }
        other => panic!("expected Oversized error, got: {other:?}"),
    }
}

#[test]
fn framing_malformed_payload_rejected_without_panic() {
    // Valid length prefix pointing to garbage data.
    let payload = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02];
    let mut buf = (payload.len() as u32).to_le_bytes().to_vec();
    buf.extend_from_slice(&payload);

    let result: Result<ClientMessage, FramingError> =
        read_message(&mut buf.as_slice(), MAX_FRAME_SIZE);
    assert!(result.is_err(), "malformed payload should be rejected");
    match result {
        Err(FramingError::Bincode(_)) => {} // expected
        other => panic!("expected Bincode error, got: {other:?}"),
    }
}

#[test]
fn framing_truncated_stream_returns_unexpected_eof() {
    // Write a length prefix claiming 100 bytes, but only provide 4.
    let mut buf: Vec<u8> = 100u32.to_le_bytes().to_vec();
    buf.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);

    let result: Result<ClientMessage, FramingError> =
        read_message(&mut buf.as_slice(), MAX_FRAME_SIZE);
    match result {
        Err(FramingError::UnexpectedEof) => {}
        other => panic!("expected UnexpectedEof, got: {other:?}"),
    }
}

#[test]
fn framing_zero_length_message() {
    // A 1-byte message (smallest possible valid bincode payload).
    // Actually, let's test with the smallest real message: Detach.
    let msg = ClientMessage::Detach;
    let mut buf = Vec::new();
    write_message(&mut buf, &msg).unwrap();

    // Verify the length prefix is correct
    let len = u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize;
    assert_eq!(
        len,
        buf.len() - 4,
        "length prefix should match payload size"
    );

    let decoded: ClientMessage = read_message(&mut buf.as_slice(), MAX_FRAME_SIZE).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn framing_partial_read_reassembly() {
    // Simulate partial reads by using a reader that yields small chunks.
    let msg = ClientMessage::Input {
        data: vec![42; 500], // 500-byte input payload
    };
    let mut full_buf = Vec::new();
    write_message(&mut full_buf, &msg).unwrap();

    // Wrap in a chunked reader that only yields 7 bytes at a time.
    let mut chunked = ChunkedReader::new(full_buf, 7);
    let decoded: ClientMessage = read_message(&mut chunked, MAX_FRAME_SIZE).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn oversized_input_rejected_custom_max() {
    // Verify a custom (small) max_frame_size is enforced.
    let msg = ClientMessage::Input {
        data: vec![0x41; 1000],
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &msg).unwrap();

    let result: Result<ClientMessage, FramingError> = read_message(&mut buf.as_slice(), 64);
    // The actual bincode payload for 1000 bytes of input will be > 64 bytes.
    assert!(
        matches!(result, Err(FramingError::Oversized { .. })),
        "expected Oversized with small max_frame_size"
    );
}

#[test]
fn read_message_rejects_trailing_bytes() {
    // Encode a valid message, then append an extra byte after it.
    let msg = ClientMessage::Detach;
    let mut payload = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let original_len = payload.len();
    payload.push(0xDE); // trailing garbage

    // Frame it with the inflated length (original + 1).
    let mut buf = (payload.len() as u32).to_le_bytes().to_vec();
    buf.extend_from_slice(&payload);

    let result: Result<ClientMessage, FramingError> =
        read_message(&mut buf.as_slice(), MAX_FRAME_SIZE);
    match result {
        Err(FramingError::Bincode(msg)) => {
            assert!(
                msg.contains("trailing bytes"),
                "error should mention trailing bytes: {msg}"
            );
            assert!(
                msg.contains(&format!("decoded {original_len}")),
                "error should mention decoded byte count: {msg}"
            );
        }
        other => panic!("expected Bincode error about trailing bytes, got: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn framing_over_unix_socketpair() {
    use std::os::unix::net::UnixStream;

    let (mut a, mut b) = UnixStream::pair().expect("socketpair");

    let messages = vec![
        ClientMessage::TerminalHello {
            version: PROTOCOL_VERSION,
            cols: 200,
            rows: 60,
            cell_width_px: 8,
            cell_height_px: 16,
            pixel_mouse: true,
        },
        ClientMessage::Input {
            data: b"hello world".to_vec(),
        },
        ClientMessage::ClipboardImage {
            target: ClientClipboardImageTarget::Pane("w1:p1".into()),
            extension: "png".to_owned(),
            data: vec![0x89, b'P', b'N', b'G'],
        },
        ClientMessage::Resize {
            cols: 100,
            rows: 30,
            cell_width_px: 8,
            cell_height_px: 16,
            pixel_mouse: true,
        },
        ClientMessage::Detach,
    ];

    // Set non-blocking so we can write and read in the same test.
    a.set_nonblocking(false).unwrap();
    b.set_nonblocking(false).unwrap();

    for msg in &messages {
        write_message(&mut a, msg).unwrap();
    }

    for expected in &messages {
        let decoded: ClientMessage = read_message(&mut b, MAX_FRAME_SIZE).unwrap();
        assert_eq!(*expected, decoded);
    }
}
