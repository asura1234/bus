//! Wire protocol for bus server/client communication.
//!
//! Defines the message types, framing, version negotiation, and safety
//! constraints for the binary protocol over local sockets.
//!
//! `PROTOCOL_VERSION` still guards same-install direct-terminal and internal
//! operations. Client-owned shells negotiate the independent stable endpoint
//! contract in [`handshake`].

pub mod handshake;

mod frame;
mod frame_adapters;
mod framing;
mod host_theme;
mod input;
mod input_adapters;
mod messages;
mod notifications;
mod shell;
mod surface;
mod version;

pub use frame::TerminalFrame;
pub use frame::{CellData, CursorState, FrameData};
pub(crate) use frame_adapters::{
    color_to_u32, modifier_to_u16, modifier_with_underline_style, underline_style_from_modifier,
};
#[cfg(test)]
use frame_adapters::{u16_to_modifier, u32_to_color};
pub use framing::{read_message, write_message, FramingError};
#[cfg(test)]
pub use host_theme::ClientHostColor;
pub use host_theme::{ClientHostAppearance, ClientHostDefaultColorKind, ClientHostThemeUpdate};
#[cfg(any(windows, test))]
pub use input::{ClientInputEvent, ClientKeySource};
pub use input::{
    ClientKeyCode, ClientKeyKind, ClientMouseButton, ClientMouseGeometry, ClientMouseKind,
    ClientMousePosition, ClientPaneInputEvent,
};
pub use messages::{ClientClipboardImageTarget, ClientMessage, ServerMessage};
pub use notifications::{
    NotifyKind, SemanticNotification, SemanticNotificationKind, SemanticNotificationSound,
};
pub use shell::{
    ClientShellAgent, ClientShellPane, ClientShellSnapshot, ClientShellTab, ClientShellWorkspace,
};
pub use surface::{
    ClientSurfaceSize, PaneSurfaceFrame, PaneSurfacePane, PaneSurfacePatch, PaneSurfacePatchRow,
    PaneSurfaceScrollMetrics, PaneSurfaceSplit, PaneSurfaceSplitDirection, RenderEncoding,
    SurfaceGraphicsAsset, SurfaceGraphicsAssetKey, SurfaceGraphicsFormat, SurfaceGraphicsPlacement,
    SurfaceGraphicsScene, SurfaceGraphicsSource, SurfaceGraphicsTarget, SurfaceRect,
};
pub use version::{
    MAX_CLIPBOARD_IMAGE_PAYLOAD, MAX_FRAME_SIZE, MAX_GRAPHICS_FRAME_SIZE, PROTOCOL_VERSION,
};

#[cfg(test)]
use std::io::{self, Read};

#[cfg(test)]
#[path = "tests/codec_test.rs"]
mod tests;

pub(crate) use surface::SurfaceGraphicsVisibility;
