use super::*;
use ratatui::style::{Color, Modifier};

// ---- Round-trip: ClientMessage ----

// ---- Round-trip: ServerMessage ----

// ---- Framing ----

// ---- Malformed/oversized input ----

// ---- FrameData ↔ ratatui Buffer conversion ----

// ---- Color conversion coverage ----

// ---- Modifier conversion ----

// ---- Unix socketpair integration test ----

// ---- Helper: chunked reader for simulating partial reads ----

/// A `Read` wrapper that yields at most `chunk_size` bytes per `read()` call,
/// simulating partial reads on a real socket.
struct ChunkedReader {
    data: Vec<u8>,
    pos: usize,
    chunk_size: usize,
}

impl ChunkedReader {
    fn new(data: Vec<u8>, chunk_size: usize) -> Self {
        Self {
            data,
            pos: 0,
            chunk_size,
        }
    }
}

impl Read for ChunkedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.data.len() {
            return Ok(0);
        }
        let remaining = self.data.len() - self.pos;
        let to_read = buf.len().min(remaining).min(self.chunk_size);
        buf[..to_read].copy_from_slice(&self.data[self.pos..self.pos + to_read]);
        self.pos += to_read;
        Ok(to_read)
    }
}

#[path = "framing_test.rs"]
mod framing;

#[path = "input_test.rs"]
mod input;

#[path = "frame_test.rs"]
mod frame;

#[path = "version_test.rs"]
mod version;

#[test]
fn client_shell_resize_roundtrip() {
    let msg = ClientMessage::ClientShellResize {
        cell_width_px: 8,
        cell_height_px: 16,
        surface_size: ClientSurfaceSize { cols: 74, rows: 29 },
        pixel_mouse: true,
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn client_shell_focus_roundtrip() {
    let message = ClientMessage::ClientShellFocus { focused: false };
    let encoded = bincode::serde::encode_to_vec(&message, bincode::config::standard())
        .expect("encode client shell focus");
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
            .expect("decode client shell focus");
    assert_eq!(decoded, message);
}

#[test]
fn client_shell_mouse_capture_roundtrip() {
    let message = ClientMessage::ClientShellMouseCapture { enabled: false };
    let encoded = bincode::serde::encode_to_vec(&message, bincode::config::standard())
        .expect("encode client shell mouse capture");
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
            .expect("decode client shell mouse capture");
    assert_eq!(decoded, message);
}

#[test]
fn client_shell_host_theme_roundtrip() {
    let message = ClientMessage::ClientShellHostTheme {
        update: ClientHostThemeUpdate::PaletteColors(vec![(
            4,
            ClientHostColor {
                r: 10,
                g: 20,
                b: 30,
            },
        )]),
    };
    let encoded = bincode::serde::encode_to_vec(&message, bincode::config::standard())
        .expect("encode host theme update");
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
            .expect("decode host theme update");
    assert_eq!(decoded, message);
}

#[test]
fn client_shell_endpoint_messages_roundtrip() {
    let request = ClientMessage::ClientShellEndpointRequest {
        boot_id: "boot-a".into(),
        request: r#"{"id":"request-a","method":"session.snapshot","params":{}}"#.into(),
    };
    let encoded = bincode::serde::encode_to_vec(&request, bincode::config::standard())
        .expect("encode endpoint request");
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
            .expect("decode endpoint request");
    assert_eq!(decoded, request);

    let response = ServerMessage::ClientShellEndpointResponseChunk {
        boot_id: "boot-a".into(),
        request_id: "request-a".into(),
        final_chunk: true,
        data: br#"{"id":"request-a","result":{"type":"ok"}}"#.to_vec(),
    };
    let encoded = bincode::serde::encode_to_vec(&response, bincode::config::standard())
        .expect("encode endpoint response");
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
            .expect("decode endpoint response");
    assert_eq!(decoded, response);
}

#[test]
fn client_clipboard_image_roundtrip() {
    let msg = ClientMessage::ClipboardImage {
        target: ClientClipboardImageTarget::Pane("w1:p1".into()),
        extension: "png".to_owned(),
        data: vec![0x89, b'P', b'N', b'G'],
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn client_resize_roundtrip() {
    let msg = ClientMessage::Resize {
        cols: 80,
        rows: 24,
        cell_width_px: 8,
        cell_height_px: 16,
        pixel_mouse: true,
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn client_detach_roundtrip() {
    let msg = ClientMessage::Detach;
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn server_shutdown_roundtrip() {
    let msg = ServerMessage::ServerShutdown {
        reason: Some("updating".to_owned()),
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn semantic_notification_roundtrip() {
    let msg = ServerMessage::SemanticNotification(SemanticNotification {
        kind: SemanticNotificationKind::NeedsAttention,
        title: "codex needs attention".into(),
        body: Some("repo · 1".into()),
        sound: Some(SemanticNotificationSound::Request),
        agent: Some("codex".into()),
        workspace_id: Some("w1".into()),
        tab_id: Some("w1:t1".into()),
        pane_id: Some("w1:p1".into()),
        position: Some(crate::utils::config::ToastHerdrPosition::TopRight),
    });
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn server_notify_roundtrip() {
    for kind in [NotifyKind::Toast, NotifyKind::SystemToast] {
        let msg = ServerMessage::Notify {
            kind,
            message: "agent done".to_owned(),
            body: None,
        };
        let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        let (decoded, _): (ServerMessage, _) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
        assert_eq!(msg, decoded);
    }
}

#[test]
fn server_clipboard_roundtrip() {
    let msg = ServerMessage::Clipboard {
        data: "dGVzdA==".to_owned(), // base64 "test"
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn server_window_title_roundtrip() {
    for title in [Some("herdr api".to_owned()), None] {
        let msg = ServerMessage::WindowTitle { title };
        let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        let (decoded, _): (ServerMessage, _) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
        assert_eq!(msg, decoded);
    }
}

#[test]
fn server_reload_sound_config_roundtrip() {
    let msg = ServerMessage::ReloadSoundConfig;
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn server_mouse_capture_roundtrip() {
    let msg = ServerMessage::MouseCapture {
        enabled: true,
        sgr_pixels: true,
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn server_terminal_bell_roundtrip() {
    let msg = ServerMessage::TerminalBell { count: 3 };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn color_roundtrip_all_named_colors() {
    let named = [
        Color::Reset,
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    for c in named {
        assert_eq!(
            u32_to_color(color_to_u32(c)),
            c,
            "roundtrip failed for {c:?}"
        );
    }
}

#[test]
fn color_roundtrip_indexed() {
    for i in 0..=255u8 {
        let c = Color::Indexed(i);
        assert_eq!(
            u32_to_color(color_to_u32(c)),
            c,
            "roundtrip failed for Indexed({i})"
        );
    }
}

#[test]
fn color_roundtrip_rgb() {
    let c = Color::Rgb(0xAB, 0xCD, 0xEF);
    assert_eq!(u32_to_color(color_to_u32(c)), c);

    let c = Color::Rgb(0, 0, 0);
    assert_eq!(u32_to_color(color_to_u32(c)), c);

    let c = Color::Rgb(255, 255, 255);
    assert_eq!(u32_to_color(color_to_u32(c)), c);
}

#[test]
fn modifier_roundtrip() {
    let all_mods = [
        Modifier::BOLD,
        Modifier::ITALIC,
        Modifier::REVERSED,
        Modifier::UNDERLINED,
        Modifier::DIM,
        Modifier::SLOW_BLINK,
        Modifier::CROSSED_OUT,
        Modifier::BOLD | Modifier::ITALIC,
        Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED,
        Modifier::empty(),
    ];
    for m in all_mods {
        assert_eq!(
            u16_to_modifier(modifier_to_u16(m)),
            m,
            "roundtrip failed for {m:?}"
        );
    }
}
