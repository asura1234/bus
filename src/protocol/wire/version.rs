/// Current protocol version. Bumped when wire format changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 22;

/// Maximum allowed frame payload size (2 MB). Frames larger than this are
/// rejected to prevent denial-of-service via oversized length prefixes.
pub const MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;

/// Maximum allowed server-to-client frame payload when Kitty graphics are enabled.
/// Normal traffic keeps `MAX_FRAME_SIZE`; this larger cap is only for explicit
/// image payloads that are naturally much larger after base64 encoding.
pub const MAX_GRAPHICS_FRAME_SIZE: usize = 32 * 1024 * 1024;

/// Maximum clipboard image payload size for remote paste bridging.
pub const MAX_CLIPBOARD_IMAGE_PAYLOAD: usize = 16 * 1024 * 1024;
