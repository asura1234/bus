mod decode;
mod encode;
pub(crate) mod host;
mod key;
pub(crate) mod mouse;
mod protocol;

pub use decode::parse_terminal_key_sequence;
pub use encode::encode_terminal_key;
#[cfg(not(windows))]
pub use key::ime_compatible_keyboard_enhancement_flags;
pub use key::WindowsKeyRecord;
pub use key::{host_modify_other_keys_mode, KeyIdentity, TerminalKey, TextCommit};
pub use protocol::KeyboardProtocol;
