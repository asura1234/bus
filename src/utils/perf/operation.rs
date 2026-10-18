//! A single atomic active-message snapshot. No payload reads, clones or formatting.
use std::sync::atomic::{AtomicU64, Ordering};

const VARIANTS: &[&str] = &[
    "idle",
    "Terminal",
    "Graphics",
    "ServerShutdown",
    "Notify",
    "Clipboard",
    "WindowTitle",
    "ReloadSoundConfig",
    "MouseCapture",
    "TerminalBell",
    "GraphicsFile",
    "GraphicsTransmissionRetired",
    "ClientShellSnapshot",
    "PaneSurface",
    "SemanticNotification",
    "ClientShellError",
    "ClientShellKeyboardReportAll",
    "ClientShellEndpointResponseChunk",
    "PaneSurfacePatch",
    "EndpointControl",
    "Welcome",
];
const PHASES: &[&str] = &[
    "idle",
    "terminal_output",
    "control",
    "effect",
    "surface",
    "endpoint",
    "surface_patch",
];
#[cfg(test)]
pub(super) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static MESSAGE: AtomicU64 = AtomicU64::new(0);

/// The wire variant and dispatch phase are small stable numeric tags. Payload
/// size is the actual decoded wire frame length (excluding its four-byte prefix).
pub(crate) fn message_enter(variant: u8, phase: u8, bytes: u32) -> MessageGuard {
    MESSAGE.store(pack(variant, phase, bytes), Ordering::Release);
    MessageGuard
}
fn pack(variant: u8, phase: u8, bytes: u32) -> u64 {
    u64::from(bytes) | (u64::from(variant) << 32) | (u64::from(phase) << 40)
}
pub(crate) struct MessageGuard;
impl Drop for MessageGuard {
    fn drop(&mut self) {
        MESSAGE.store(0, Ordering::Release);
    }
}
pub(super) fn snapshot() -> Option<serde_json::Value> {
    let value = MESSAGE.load(Ordering::Acquire);
    let variant = (value >> 32) as u8;
    (variant != 0).then(|| serde_json::json!({
        "variant": VARIANTS.get(variant as usize).copied().unwrap_or("unknown"),
        "phase": PHASES.get((value >> 40) as usize).copied().unwrap_or("unknown"), "payload_bytes": value as u32,
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn publishes_one_coherent_message_and_clears_on_return() {
        let _serial = super::TEST_LOCK.lock().unwrap();
        let guard = super::message_enter(4, 2, 12345);
        assert_eq!(
            super::snapshot(),
            Some(serde_json::json!({"variant":"Notify","phase":"control","payload_bytes":12345}))
        );
        drop(guard);
        assert_eq!(super::snapshot(), None);
    }
}
