use super::*;

fn key_char(ch: char) -> WindowsInputRecord {
    WindowsInputRecord::Key(WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: 0,
        virtual_scan_code: 0,
        unicode: ch as u16,
        control_key_state: 0,
    })
}

fn key_vk(vk: u16, control_key_state: u32) -> WindowsInputRecord {
    key_vk_with_unicode(vk, '\0', control_key_state)
}

fn key_vk_with_unicode(vk: u16, ch: char, control_key_state: u32) -> WindowsInputRecord {
    WindowsInputRecord::Key(WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: vk,
        virtual_scan_code: 0,
        unicode: ch as u16,
        control_key_state,
    })
}

fn key_vk_with_utf16(vk: u16, unit: u16) -> WindowsInputRecord {
    WindowsInputRecord::Key(WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: vk,
        virtual_scan_code: 0,
        unicode: unit,
        control_key_state: 0,
    })
}

fn key_vk_with_utf16_mods(vk: u16, unit: u16, control_key_state: u32) -> WindowsInputRecord {
    WindowsInputRecord::Key(WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: vk,
        virtual_scan_code: 0,
        unicode: unit,
        control_key_state,
    })
}

fn key_vk_with_scan_unicode(
    vk: u16,
    scan: u16,
    ch: char,
    control_key_state: u32,
) -> WindowsInputRecord {
    WindowsInputRecord::Key(WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: vk,
        virtual_scan_code: scan,
        unicode: ch as u16,
        control_key_state,
    })
}

fn key_vk_with_repeat(vk: u16, repeat_count: u16) -> WindowsInputRecord {
    WindowsInputRecord::Key(WindowsKeyRecord {
        key_down: true,
        repeat_count,
        virtual_key_code: vk,
        virtual_scan_code: 1,
        unicode: 0,
        control_key_state: 0,
    })
}

fn key_vk_with_unicode_repeat(
    vk: u16,
    ch: char,
    control_key_state: u32,
    repeat_count: u16,
) -> WindowsInputRecord {
    WindowsInputRecord::Key(WindowsKeyRecord {
        key_down: true,
        repeat_count,
        virtual_key_code: vk,
        virtual_scan_code: 0,
        unicode: ch as u16,
        control_key_state,
    })
}

fn key_vk_up_with_unicode(vk: u16, ch: char, control_key_state: u32) -> WindowsInputRecord {
    WindowsInputRecord::Key(WindowsKeyRecord {
        key_down: false,
        repeat_count: 1,
        virtual_key_code: vk,
        virtual_scan_code: 0,
        unicode: ch as u16,
        control_key_state,
    })
}

fn key_vk_up_with_scan_unicode(
    vk: u16,
    scan: u16,
    ch: char,
    control_key_state: u32,
) -> WindowsInputRecord {
    WindowsInputRecord::Key(WindowsKeyRecord {
        key_down: false,
        repeat_count: 1,
        virtual_key_code: vk,
        virtual_scan_code: scan,
        unicode: ch as u16,
        control_key_state,
    })
}

fn translate(
    records: impl IntoIterator<Item = WindowsInputRecord>,
) -> Vec<crate::protocol::ClientInputEvent> {
    semantic_only(translate_with_provenance(records))
}

fn semantic_only(
    events: impl IntoIterator<Item = crate::protocol::ClientInputEvent>,
) -> Vec<crate::protocol::ClientInputEvent> {
    events
        .into_iter()
        .map(|event| match event {
            crate::protocol::ClientInputEvent::Key {
                code,
                modifiers,
                kind,
                repeat_count,
                generated_text,
                ..
            } => crate::protocol::ClientInputEvent::Key {
                code,
                modifiers,
                kind,
                repeat_count,
                generated_text,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
            event => event,
        })
        .collect()
}

fn translate_with_provenance(
    records: impl IntoIterator<Item = WindowsInputRecord>,
) -> Vec<crate::protocol::ClientInputEvent> {
    let mut translator = WindowsInputTranslator::default();
    records
        .into_iter()
        .flat_map(|record| translator.translate(record))
        .collect()
}

fn win32_input_mode_encoded_raw_bytes(bytes: &[u8]) -> Vec<WindowsInputRecord> {
    bytes
        .iter()
        .flat_map(|byte| {
            format!("\x1b[0;0;{byte};1;0;1_")
                .chars()
                .map(key_char)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn win32_input_mode_encoded_key_bytes(bytes: &[u8]) -> Vec<WindowsInputRecord> {
    bytes
        .iter()
        .flat_map(|byte| {
            let vk = match *byte {
                b'\x1b' => 0x1b,
                b'\r' => 0x0d,
                b'[' => 0xdb,
                b'~' => 0xc0,
                b'0'..=b'9' | b'A'..=b'Z' => u16::from(*byte),
                b'a'..=b'z' => u16::from(byte.to_ascii_uppercase()),
                _ => 0,
            };
            format!("\x1b[{vk};0;{byte};1;0;1_")
                .chars()
                .map(key_char)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn win32_input_mode_encoded_record(record: WindowsKeyRecord) -> Vec<WindowsInputRecord> {
    format!(
        "\x1b[{};{};{};{};{};{}_",
        record.virtual_key_code,
        record.virtual_scan_code,
        record.unicode,
        u16::from(record.key_down),
        record.control_key_state,
        record.repeat_count,
    )
    .chars()
    .map(key_char)
    .collect()
}

#[cfg(windows)]
#[path = "handoff_test.rs"]
mod handoff;
#[path = "keymap_test.rs"]
mod keymap;
#[path = "mapper_test.rs"]
mod mapper;
#[path = "pump_test.rs"]
mod pump;
#[path = "win32_input_mode_test.rs"]
mod win32_input_mode;
