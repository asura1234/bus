use super::*;

#[test]
fn client_hello_roundtrip() {
    let msg = ClientMessage::TerminalHello {
        version: PROTOCOL_VERSION,
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
fn client_shell_hello_roundtrip() {
    let msg = ClientMessage::ClientShellHello {
        version: PROTOCOL_VERSION,
        cell_width_px: 8,
        cell_height_px: 16,
        surface_size: ClientSurfaceSize { cols: 80, rows: 29 },
        pixel_mouse: true,
        direct_graphics: false,
        endpoint_keybindings: true,
        mouse_capture: true,
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn endpoint_control_roundtrip() {
    let msg = ClientMessage::EndpointControl {
        kind: "endpoint.hello.v1".into(),
        data: r#"{"generation":1}"#.into(),
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn server_welcome_roundtrip() {
    let msg = ServerMessage::Welcome {
        version: PROTOCOL_VERSION,
        encoding: RenderEncoding::SemanticFrame,
        error: None,
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn server_welcome_with_error_roundtrip() {
    let msg = ServerMessage::Welcome {
        version: PROTOCOL_VERSION,
        encoding: RenderEncoding::SemanticFrame,
        error: Some("incompatible version".to_owned()),
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}
