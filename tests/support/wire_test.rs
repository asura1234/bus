use super::*;
use std::io::{Read, Write};
use std::process::Command;

pub const CURRENT_ENDPOINT_PROTOCOL_GENERATION: u32 = 1;

pub const SERVER_MESSAGE_SERVER_SHUTDOWN: u32 = 3;

pub const SERVER_MESSAGE_ENDPOINT_CONTROL: u32 = 19;

pub const SERVER_MESSAGE_PANE_SURFACE: u32 = 13;

pub const SERVER_MESSAGE_SEMANTIC_NOTIFICATION: u32 = 14;

pub const SERVER_MESSAGE_PANE_SURFACE_PATCH: u32 = 18;

const CLIENT_MESSAGE_CLIENT_SHELL_FOCUS: u32 = 12;

const CLIENT_MESSAGE_ENDPOINT_CONTROL: u32 = 14;

fn encode_varint_u32(v: u32) -> Vec<u8> {
    if v < 251 {
        vec![v as u8]
    } else if v < 65536 {
        let mut buf = vec![251u8];
        buf.extend_from_slice(&(v as u16).to_le_bytes());
        buf
    } else {
        let mut buf = vec![252u8];
        buf.extend_from_slice(&v.to_le_bytes());
        buf
    }
}

fn frame_message(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut framed = len.to_le_bytes().to_vec();
    framed.extend_from_slice(payload);
    framed
}

fn decode_varint_u32(payload: &[u8], offset: usize) -> Result<(u32, usize), String> {
    if offset >= payload.len() {
        return Err("payload too short for varint".into());
    }
    let first_byte = payload[offset];
    match first_byte {
        0..=250 => Ok((first_byte as u32, 1)),
        251 => {
            if offset + 3 > payload.len() {
                return Err("payload too short for u16 varint".into());
            }
            let v = u16::from_le_bytes(
                payload[offset + 1..offset + 3]
                    .try_into()
                    .map_err(|e: std::array::TryFromSliceError| e.to_string())?,
            );
            Ok((v as u32, 3))
        }
        252 => {
            if offset + 5 > payload.len() {
                return Err("payload too short for u32 varint".into());
            }
            let v = u32::from_le_bytes(
                payload[offset + 1..offset + 5]
                    .try_into()
                    .map_err(|e: std::array::TryFromSliceError| e.to_string())?,
            );
            Ok((v, 5))
        }
        _ => Err(format!("unsupported varint tag: {first_byte}")),
    }
}

fn encode_varint_enum(variant_idx: u32, fields: &[&[u8]]) -> Vec<u8> {
    let mut buf = encode_varint_u32(variant_idx);
    for field in fields {
        buf.extend_from_slice(field);
    }
    buf
}

fn encode_string(value: &str) -> Vec<u8> {
    let mut encoded = encode_varint_u32(value.len() as u32);
    encoded.extend_from_slice(value.as_bytes());
    encoded
}

fn decode_string(payload: &[u8], offset: &mut usize) -> Result<String, String> {
    let (len, consumed) = decode_varint_u32(payload, *offset)?;
    *offset += consumed;
    let len = len as usize;
    if *offset + len > payload.len() {
        return Err("payload too short for string content".into());
    }
    let value = String::from_utf8(payload[*offset..*offset + len].to_vec())
        .map_err(|err| err.to_string())?;
    *offset += len;
    Ok(value)
}

fn read_handshake_response(
    stream: &mut UnixStream,
    hello_payload: &[u8],
) -> Result<Vec<u8>, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(&frame_message(hello_payload))
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).map_err(|e| e.to_string())?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 2 * 1024 * 1024 {
        return Err(format!("oversized response: {len}"));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).map_err(|e| e.to_string())?;
    Ok(payload)
}

pub fn client_shell_handshake(
    stream: &mut UnixStream,
    endpoint_generation: u32,
    surface_cols: u16,
    surface_rows: u16,
) -> Result<(u32, Option<String>), String> {
    let data = serde_json::json!({
        "generation": endpoint_generation,
        "client_version": current_build_version(),
        "cell_width_px": 8,
        "cell_height_px": 16,
        "surface_size": {"cols": surface_cols, "rows": surface_rows},
        "pixel_mouse": false,
        "direct_graphics": false,
        "endpoint_keybindings": false,
        "mouse_capture": false,
        "surface_active": true,
        "snapshot_codecs": ["shell.snapshot.v1"],
        "surface_codecs": ["shell.surface.v1"],
        "input_codecs": ["shell.input.semantic.v1"],
        "blob_codecs": ["shell.blob.v1"]
    })
    .to_string();
    let hello_payload = encode_varint_enum(
        CLIENT_MESSAGE_ENDPOINT_CONTROL,
        &[&encode_string("endpoint.hello.v1"), &encode_string(&data)],
    );
    let response = read_handshake_response(stream, &hello_payload)?;
    let mut offset = 0;
    let (variant, consumed) = decode_varint_u32(&response, offset)?;
    offset += consumed;
    if variant != SERVER_MESSAGE_ENDPOINT_CONTROL {
        return Err(format!(
            "expected EndpointControl (variant {SERVER_MESSAGE_ENDPOINT_CONTROL}), got variant {variant}"
        ));
    }
    let kind = decode_string(&response, &mut offset)?;
    if kind != "endpoint.welcome.v1" {
        return Err(format!("expected endpoint.welcome.v1, got {kind}"));
    }
    let data = decode_string(&response, &mut offset)?;
    let value: serde_json::Value = serde_json::from_str(&data).map_err(|err| err.to_string())?;
    let generation = value["generation"]
        .as_u64()
        .ok_or_else(|| "endpoint welcome omitted generation".to_owned())?
        as u32;
    let error = value["error"]
        .as_object()
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Ok((generation, error))
}

fn current_build_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let output = Command::new(env!("CARGO_BIN_EXE_bus"))
            .arg("--version")
            .output()
            .expect("read the integration-test binary's build version");
        assert!(output.status.success(), "bus --version failed: {output:?}");
        let version = String::from_utf8(output.stdout).expect("bus --version should emit UTF-8");
        version
            .trim()
            .strip_prefix("bus ")
            .expect("bus --version should include the binary name")
            .to_owned()
    })
}

pub fn read_server_message(stream: &mut UnixStream) -> Result<(u32, Vec<u8>), String> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .map_err(|e| format!("read length prefix: {e}"))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 2 * 1024 * 1024 {
        return Err(format!("oversized frame: {len} bytes"));
    }
    if len == 0 {
        return Err("zero-length frame".into());
    }

    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .map_err(|e| format!("read payload: {e}"))?;

    let (variant, consumed) = decode_varint_u32(&payload, 0)?;
    Ok((variant, payload[consumed..].to_vec()))
}

pub fn send_client_shell_focus(stream: &mut UnixStream, focused: bool) -> Result<(), String> {
    let mut payload = encode_varint_u32(CLIENT_MESSAGE_CLIENT_SHELL_FOCUS);
    payload.push(u8::from(focused));
    stream
        .write_all(&frame_message(&payload))
        .map_err(|e| format!("write client shell focus: {e}"))?;
    stream
        .flush()
        .map_err(|e| format!("flush client shell focus: {e}"))
}

pub fn send_detach(stream: &mut UnixStream) -> Result<(), String> {
    let detach_payload = encode_varint_u32(4);
    let framed = frame_message(&detach_payload);
    stream
        .write_all(&framed)
        .map_err(|e| format!("write detach: {e}"))?;
    stream.flush().map_err(|e| format!("flush detach: {e}"))?;
    Ok(())
}

pub fn drain_messages(stream: &mut UnixStream) {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    while read_server_message(stream).is_ok() {}
    stream.set_read_timeout(None).unwrap();
}

pub fn wait_until<F>(timeout: Duration, interval: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        thread::sleep(interval);
    }
    predicate()
}

pub fn wait_for_message_variant(
    stream: &mut UnixStream,
    timeout: Duration,
    variant: u32,
) -> Result<bool, String> {
    wait_for_message_variants(stream, timeout, &[variant])
}

pub fn wait_for_message_variants(
    stream: &mut UnixStream,
    timeout: Duration,
    variants: &[u32],
) -> Result<bool, String> {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match read_server_message(stream) {
            Ok((got, _)) if variants.contains(&got) => return Ok(true),
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    Ok(false)
}

pub fn wait_for_client_shell_bootstrap(
    stream: &mut UnixStream,
    timeout: Duration,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    let mut saw_snapshot = false;
    while Instant::now() < deadline {
        match read_server_message(stream) {
            Ok((SERVER_MESSAGE_ENDPOINT_CONTROL, payload)) => {
                let mut offset = 0;
                if decode_string(&payload, &mut offset).as_deref() == Ok("shell.snapshot.v1") {
                    saw_snapshot = true;
                }
            }
            Ok((SERVER_MESSAGE_PANE_SURFACE, _)) if saw_snapshot => return Ok(()),
            Ok((SERVER_MESSAGE_PANE_SURFACE, _)) => {
                return Err("client shell pane surface arrived before its snapshot".into());
            }
            Ok(_) | Err(_) => {}
        }
    }
    Err(format!(
        "timed out waiting for client shell {}",
        if saw_snapshot {
            "pane surface"
        } else {
            "snapshot"
        }
    ))
}

pub fn wait_for_disconnect(stream: &mut UnixStream, timeout: Duration) -> Result<bool, String> {
    stream.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    let mut idle_since = None;
    let result = loop {
        match read_server_message(stream) {
            Ok(_) => idle_since = None,
            Err(err)
                if err.to_ascii_lowercase().contains("would block")
                    || err.contains("Resource temporarily unavailable") =>
            {
                let idle_started = *idle_since.get_or_insert_with(Instant::now);
                if idle_started.elapsed() >= Duration::from_millis(200) {
                    break Ok(true);
                }
            }
            Err(_) => break Ok(true),
        }
        if Instant::now() >= deadline {
            break Ok(false);
        }
        thread::sleep(Duration::from_millis(25));
    };
    let _ = stream.set_nonblocking(false);
    result
}
