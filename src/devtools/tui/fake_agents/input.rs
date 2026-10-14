//! Raw terminal input as the fake providers read it.

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Input {
    Text(String),
    Backspace,
    /// Ctrl+K or Ctrl+U.
    KillLine,
    Enter,
    /// A lone Esc, as Bus sends the key in one write.
    Escape,
    Interrupt,
}

/// Splits raw terminal input into text, Enter, Backspace and Ctrl+C/D,
/// unwrapping bracketed pastes and dropping other escape sequences. Bytes of
/// an unfinished sequence stay in `pending`.
pub(super) fn drain_input(pending: &mut Vec<u8>) -> Vec<Input> {
    let mut events = Vec::new();
    let mut text = Vec::new();
    let mut i = 0;
    let flush = |text: &mut Vec<u8>, events: &mut Vec<Input>| {
        if !text.is_empty() {
            events.push(Input::Text(String::from_utf8_lossy(text).into_owned()));
            text.clear();
        }
    };
    while i < pending.len() {
        let rest = &pending[i..];
        if rest.starts_with(b"\x1b[200~") {
            let Some(end) = find(rest, b"\x1b[201~") else {
                break;
            };
            text.extend_from_slice(&rest[6..end]);
            i += end + 6;
            continue;
        }
        match rest[0] {
            // Claude Code submits on CR; a typed LF (Ctrl+J) is a newline.
            b'\r' => {
                flush(&mut text, &mut events);
                events.push(Input::Enter);
                i += 1;
            }
            b'\n' => {
                text.push(b'\n');
                i += 1;
            }
            0x7f | 0x08 => {
                flush(&mut text, &mut events);
                events.push(Input::Backspace);
                i += 1;
            }
            0x03 | 0x04 => {
                flush(&mut text, &mut events);
                events.push(Input::Interrupt);
                i += 1;
            }
            // Ctrl+K and Ctrl+U: Bus withdraws a draft with them.
            0x0b | 0x15 => {
                flush(&mut text, &mut events);
                events.push(Input::KillLine);
                i += 1;
            }
            0x1b => {
                // CSI sequences end at a byte in 0x40..=0x7e; `CSI 13 u` is Enter.
                if rest.len() < 2 {
                    flush(&mut text, &mut events);
                    events.push(Input::Escape);
                    i += 1;
                    continue;
                }
                if rest[1] != b'[' {
                    i += 2;
                    continue;
                }
                let Some(end) = rest[2..].iter().position(|b| (0x40..=0x7e).contains(b)) else {
                    break;
                };
                let sequence = &rest[..end + 3];
                if sequence == b"\x1b[13u" {
                    flush(&mut text, &mut events);
                    events.push(Input::Enter);
                }
                i += end + 3;
            }
            byte if byte < 0x20 && byte != b'\t' => i += 1,
            _ => {
                text.push(rest[0]);
                i += 1;
            }
        }
    }
    // A character split across reads waits in `pending` for its last bytes.
    if i == pending.len() {
        let split = text.len() - super::super::trace::complete_utf8_len(&text);
        text.truncate(text.len() - split);
        i -= split;
    }
    flush(&mut text, &mut events);
    pending.drain(..i);
    events
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
