//! A single atomic active-message snapshot. No payload reads, clones or formatting.
use std::sync::atomic::{AtomicU64, Ordering};

// One declaration owns both the packed discriminant and its JSON name.
macro_rules! tags {
    ($kind:ident, $names:ident, {$($variant:ident => $name:literal),+ $(,)?}) => {
        #[derive(Clone, Copy)]
        #[repr(u8)]
        pub(crate) enum $kind { $($variant),+ }
        const $names: &[&str] = &[$($name),+];
    };
}
tags!(Variant, VARIANTS, {
    Idle => "idle",
    Terminal => "Terminal",
    Graphics => "Graphics",
    ServerShutdown => "ServerShutdown",
    Notify => "Notify",
    Clipboard => "Clipboard",
    WindowTitle => "WindowTitle",
    ReloadSoundConfig => "ReloadSoundConfig",
    MouseCapture => "MouseCapture",
    TerminalBell => "TerminalBell",
    GraphicsFile => "GraphicsFile",
    GraphicsTransmissionRetired => "GraphicsTransmissionRetired",
    ClientShellSnapshot => "ClientShellSnapshot",
    PaneSurface => "PaneSurface",
    SemanticNotification => "SemanticNotification",
    ClientShellError => "ClientShellError",
    ClientShellKeyboardReportAll => "ClientShellKeyboardReportAll",
    ClientShellEndpointResponseChunk => "ClientShellEndpointResponseChunk",
    PaneSurfacePatch => "PaneSurfacePatch",
    EndpointControl => "EndpointControl",
    Welcome => "Welcome",
});
tags!(Phase, PHASES, {
    Idle => "idle",
    TerminalOutput => "terminal_output",
    Control => "control",
    Effect => "effect",
    Surface => "surface",
    Endpoint => "endpoint",
    SurfacePatch => "surface_patch",
});
#[cfg(test)]
pub(super) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static MESSAGE: AtomicU64 = AtomicU64::new(0);

/// The wire variant and dispatch phase are small stable numeric tags. Payload
/// size is the actual decoded wire frame length (excluding its four-byte prefix).
pub(crate) fn message_enter(variant: Variant, phase: Phase, bytes: u32) -> MessageGuard {
    MESSAGE.store(pack(variant, phase, bytes), Ordering::Release);
    MessageGuard
}
fn pack(variant: Variant, phase: Phase, bytes: u32) -> u64 {
    u64::from(bytes) | (u64::from(variant as u8) << 32) | (u64::from(phase as u8) << 40)
}
pub(crate) struct MessageGuard;
impl Drop for MessageGuard {
    fn drop(&mut self) {
        MESSAGE.store(pack(Variant::Idle, Phase::Idle, 0), Ordering::Release);
    }
}
pub(super) fn snapshot() -> Option<serde_json::Value> {
    let value = MESSAGE.load(Ordering::Acquire);
    let variant = (value >> 32) as u8;
    (variant != 0).then(|| {
        serde_json::json!({
            "variant": VARIANTS[variant as usize],
            "phase": PHASES[(value >> 40) as usize], "payload_bytes": value as u32,
        })
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn publishes_one_coherent_message_and_clears_on_return() {
        let _serial = super::TEST_LOCK.lock().unwrap();
        let guard = super::message_enter(super::Variant::Notify, super::Phase::Control, 12345);
        assert_eq!(
            super::snapshot(),
            Some(serde_json::json!({"variant":"Notify","phase":"control","payload_bytes":12345}))
        );
        drop(guard);
        assert_eq!(super::snapshot(), None);
    }
}
