//! Streaming Win32-input-mode CSI key records and timeout flushes.
use crate::protocol::keys::WindowsKeyRecord;

#[derive(Default)]
pub(super) struct WindowsWin32InputModeFramer {
    buffer: Vec<u8>,
}

pub(super) enum WindowsWin32InputModeItem {
    Bytes(Vec<u8>),
    Key {
        bytes: Vec<u8>,
        record: WindowsKeyRecord,
    },
}

impl WindowsWin32InputModeFramer {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<WindowsWin32InputModeItem> {
        self.buffer.extend_from_slice(bytes);

        let mut items = Vec::new();
        while let Some(item) = self.next_item() {
            items.push(item);
        }
        items
    }

    fn next_item(&mut self) -> Option<WindowsWin32InputModeItem> {
        if self.buffer.is_empty() {
            return None;
        }

        if self.buffer.as_slice() == b"\x1b" || self.buffer.as_slice() == b"\x1b[" {
            return None;
        }

        if self.buffer.starts_with(b"\x1b[") {
            let mut cursor = 2;
            while cursor < self.buffer.len()
                && (self.buffer[cursor].is_ascii_digit() || self.buffer[cursor] == b';')
            {
                cursor += 1;
            }

            if cursor == self.buffer.len() {
                return None;
            }

            if self.buffer[cursor] == b'_' {
                let bytes = self.buffer[..=cursor].to_vec();
                let body = String::from_utf8_lossy(&self.buffer[2..cursor]);
                let item = parse_win32_input_mode_key_record(&body).map(|record| {
                    WindowsWin32InputModeItem::Key {
                        bytes: bytes.clone(),
                        record,
                    }
                });
                self.buffer.drain(..=cursor);
                return Some(item.unwrap_or(WindowsWin32InputModeItem::Bytes(bytes)));
            }
        }

        Some(WindowsWin32InputModeItem::Bytes(vec![self
            .buffer
            .remove(0)]))
    }

    pub(super) fn flush_timeout(&mut self) -> Vec<Vec<u8>> {
        if self.buffer.is_empty() {
            Vec::new()
        } else {
            vec![std::mem::take(&mut self.buffer)]
        }
    }
}

fn parse_win32_input_mode_key_record(body: &str) -> Option<WindowsKeyRecord> {
    let mut fields = body.split(';');
    let virtual_key_code = fields.next()?.parse::<u16>().ok()?;
    let virtual_scan_code = fields.next()?.parse::<u16>().ok()?;
    let unicode = fields.next()?.parse::<u16>().ok()?;
    let key_down = fields.next()?.parse::<u16>().ok()? != 0;
    let control_key_state = fields.next()?.parse::<u32>().ok()?;
    let repeat_count = fields.next()?.parse::<u16>().ok()?;
    if fields.next().is_some() {
        return None;
    }

    Some(WindowsKeyRecord {
        key_down,
        repeat_count,
        virtual_key_code,
        virtual_scan_code,
        unicode,
        control_key_state,
    })
}
