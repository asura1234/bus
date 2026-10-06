#[derive(Debug, Clone, Default)]
pub(crate) struct KittyKeyboardTracker {
    pending: Vec<u8>,
    modify_other_keys_level: u8,
}

impl KittyKeyboardTracker {
    pub(crate) fn observe(&mut self, bytes: &[u8]) {
        let combined;
        let bytes = if self.pending.is_empty() {
            bytes
        } else {
            combined = self
                .pending
                .iter()
                .copied()
                .chain(bytes.iter().copied())
                .collect::<Vec<_>>();
            self.pending.clear();
            &combined
        };
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != 0x1b {
                index += 1;
                continue;
            }
            if index + 1 >= bytes.len() {
                self.store_pending(&bytes[index..]);
                break;
            }
            if bytes[index + 1] == b'c' {
                self.modify_other_keys_level = 0;
            }
            if bytes[index + 1] != b'[' {
                index += 1;
                continue;
            }

            let mut end = index + 2;
            while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                end += 1;
            }
            if end >= bytes.len() {
                self.store_pending(&bytes[index..]);
                break;
            }

            match bytes[end] {
                b'm' => self.observe_modify_other_keys(&bytes[index + 2..end]),
                #[cfg(windows)]
                b'n' if bytes[index + 2..end]
                    .strip_prefix(b">")
                    .is_some_and(|params| {
                        !params.contains(&b';') && parse_kitty_keyboard_flags(params) == 4
                    }) =>
                {
                    self.modify_other_keys_level = 0;
                }
                _ => {}
            }
            index = end + 1;
        }
    }

    pub(crate) fn modify_other_keys_level(&self) -> u8 {
        self.modify_other_keys_level
    }

    #[cfg(windows)]
    pub(crate) fn modify_other_keys_enabled(&self) -> bool {
        self.modify_other_keys_level > 0
    }

    fn observe_modify_other_keys(&mut self, params: &[u8]) {
        let Some(params) = params.strip_prefix(b">") else {
            return;
        };
        if params.is_empty() {
            self.modify_other_keys_level = 0;
            return;
        }

        let mut parts = params.split(|byte| *byte == b';');
        let resource = parts.next().unwrap_or_default();
        let value = parts.next();
        if parts.next().is_some() {
            return;
        }
        if parse_kitty_keyboard_flags(resource) == 4 {
            self.modify_other_keys_level =
                value.map(parse_kitty_keyboard_flags).unwrap_or(0).min(2) as u8;
        }
    }

    fn store_pending(&mut self, bytes: &[u8]) {
        self.pending.clear();
        if bytes.len() <= 64 {
            self.pending.extend_from_slice(bytes);
        }
    }
}

fn parse_kitty_keyboard_flags(bytes: &[u8]) -> u16 {
    let first_param = bytes.split(|byte| *byte == b';').next().unwrap_or_default();
    std::str::from_utf8(first_param)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_split_csi_sequences() {
        let mut tracker = KittyKeyboardTracker::default();

        tracker.observe(b"\x1b[>1u\x1b[>4;");
        tracker.observe(b"01m\x1b[>5u\x1b[<");
        tracker.observe(b"u");

        assert_eq!(tracker.modify_other_keys_level(), 1);
        #[cfg(windows)]
        {
            assert!(tracker.modify_other_keys_enabled());
            tracker.observe(b"\x1b[>1m");
            assert!(tracker.modify_other_keys_enabled());
            tracker.observe(b"\x1b[>04n");
            assert!(!tracker.modify_other_keys_enabled());
        }
    }

    #[test]
    fn ris_clears_modify_other_keys() {
        let mut tracker = KittyKeyboardTracker::default();
        tracker.observe(b"\x1b[>1u\x1b[>5u\x1b[>4;2m\x1bc");

        assert_eq!(tracker.modify_other_keys_level(), 0);
    }

    #[test]
    fn tracks_and_replays_exact_modify_other_keys_level() {
        let mut tracker = KittyKeyboardTracker::default();

        tracker.observe(b"\x1b[>4;1m");
        assert_eq!(tracker.modify_other_keys_level(), 1);

        tracker.observe(b"\x1b[>4;2m");
        assert_eq!(tracker.modify_other_keys_level(), 2);
        tracker.observe(b"\x1b[>4;0m");
        assert_eq!(tracker.modify_other_keys_level(), 0);
    }
}
