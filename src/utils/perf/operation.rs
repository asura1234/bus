//! A single atomic active-message snapshot. No payload reads, clones or formatting.
use std::sync::atomic::{AtomicU64, Ordering};

// Names are keyed by typed discriminants, never by a second positional table.
// A new enum member without its display name fails at compile time.
const fn complete_names<const N: usize>(names: [&'static str; N]) -> [&'static str; N] {
    let mut index = 0;
    while index < N {
        assert!(!names[index].is_empty());
        index += 1;
    }
    names
}
#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum Variant {
    Idle,
    Terminal,
    Graphics,
    ServerShutdown,
    Notify,
    Clipboard,
    WindowTitle,
    ReloadSoundConfig,
    MouseCapture,
    TerminalBell,
    GraphicsFile,
    GraphicsTransmissionRetired,
    ClientShellSnapshot,
    PaneSurface,
    SemanticNotification,
    ClientShellError,
    ClientShellKeyboardReportAll,
    ClientShellEndpointResponseChunk,
    PaneSurfacePatch,
    EndpointControl,
    Welcome,
    Count,
}
const VARIANTS: [&str; Variant::Count as usize] = {
    let mut names = [""; Variant::Count as usize];
    names[Variant::Idle as usize] = "idle";
    names[Variant::Terminal as usize] = "Terminal";
    names[Variant::Graphics as usize] = "Graphics";
    names[Variant::ServerShutdown as usize] = "ServerShutdown";
    names[Variant::Notify as usize] = "Notify";
    names[Variant::Clipboard as usize] = "Clipboard";
    names[Variant::WindowTitle as usize] = "WindowTitle";
    names[Variant::ReloadSoundConfig as usize] = "ReloadSoundConfig";
    names[Variant::MouseCapture as usize] = "MouseCapture";
    names[Variant::TerminalBell as usize] = "TerminalBell";
    names[Variant::GraphicsFile as usize] = "GraphicsFile";
    names[Variant::GraphicsTransmissionRetired as usize] = "GraphicsTransmissionRetired";
    names[Variant::ClientShellSnapshot as usize] = "ClientShellSnapshot";
    names[Variant::PaneSurface as usize] = "PaneSurface";
    names[Variant::SemanticNotification as usize] = "SemanticNotification";
    names[Variant::ClientShellError as usize] = "ClientShellError";
    names[Variant::ClientShellKeyboardReportAll as usize] = "ClientShellKeyboardReportAll";
    names[Variant::ClientShellEndpointResponseChunk as usize] = "ClientShellEndpointResponseChunk";
    names[Variant::PaneSurfacePatch as usize] = "PaneSurfacePatch";
    names[Variant::EndpointControl as usize] = "EndpointControl";
    names[Variant::Welcome as usize] = "Welcome";
    complete_names(names)
};

#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum Phase {
    Idle,
    TerminalOutput,
    Control,
    Effect,
    Surface,
    Endpoint,
    SurfacePatch,
    Count,
}
const PHASES: [&str; Phase::Count as usize] = {
    let mut names = [""; Phase::Count as usize];
    names[Phase::Idle as usize] = "idle";
    names[Phase::TerminalOutput as usize] = "terminal_output";
    names[Phase::Control as usize] = "control";
    names[Phase::Effect as usize] = "effect";
    names[Phase::Surface as usize] = "surface";
    names[Phase::Endpoint as usize] = "endpoint";
    names[Phase::SurfacePatch as usize] = "surface_patch";
    complete_names(names)
};
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
