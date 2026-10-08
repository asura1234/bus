use super::frame::{CellData, CursorState, FrameData};
use serde::{Deserialize, Serialize};

/// Render payload encoding negotiated during client handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderEncoding {
    /// Send full semantic FrameData values. This is the local/default mode.
    SemanticFrame,
    /// Send already-diffed terminal ANSI byte streams.
    TerminalAnsi,
}

/// Size of the pane surface requested by a client-owned shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientSurfaceSize {
    pub cols: u16,
    pub rows: u16,
}

/// Origin-relative geometry for one pane in a rendered pane surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfacePane {
    pub pane_id: String,
    pub content_revision: u64,
    pub rect: SurfaceRect,
    pub inner_rect: SurfaceRect,
    pub scrollbar_rect: Option<SurfaceRect>,
    pub scroll: Option<PaneSurfaceScrollMetrics>,
    pub focused: bool,
    pub mouse_reporting: bool,
    pub sgr_pixel_mouse: bool,
    pub alternate_screen_active: bool,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfaceScrollMetrics {
    pub offset_from_bottom: u64,
    pub max_offset_from_bottom: u64,
    pub viewport_rows: u64,
}

/// One draggable BSP split handle relative to a pane surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfaceSplit {
    pub direction: PaneSurfaceSplitDirection,
    pub pos: u16,
    pub area: SurfaceRect,
    pub hit_rect: SurfaceRect,
    pub path: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneSurfaceSplitDirection {
    Horizontal,
    Vertical,
}

/// Wire-safe rectangle relative to a pane surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceGraphicsTarget {
    Pane { pane_id: String },
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceGraphicsSource {
    Terminal {
        target: SurfaceGraphicsTarget,
        image_id: u32,
    },
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceGraphicsFormat {
    Rgb,
    Rgba,
    Png,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsAssetKey {
    pub source: SurfaceGraphicsSource,
    pub image_width: u32,
    pub image_height: u32,
    pub format: SurfaceGraphicsFormat,
    pub data_len: u64,
    pub data_fingerprint: u64,
}

/// Image bytes newly needed by this connection's complete desired scene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsAsset {
    pub key: SurfaceGraphicsAssetKey,
    pub data: Vec<u8>,
}

/// One already-clipped desired placement relative to its target surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsPlacement {
    pub asset: SurfaceGraphicsAssetKey,
    pub logical_placement_id: u32,
    pub x: u16,
    pub y: u16,
    pub cols: u32,
    pub rows: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub z: i32,
    pub scrollback_offset: u32,
}

/// Complete desired placements plus only the image bytes not already sent for
/// the current live scene on this connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceGraphicsScene {
    pub assets: Vec<SurfaceGraphicsAsset>,
    pub placements: Vec<SurfaceGraphicsPlacement>,
}

/// One server-rendered active-tab surface without sidebar, tab bar, or overlays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfaceFrame {
    /// Endpoint process identity that produced this surface.
    pub boot_id: String,
    /// Projection revision whose focused IDs and topology produced this surface.
    pub projection_revision: u64,
    /// Monotonic revision for full surfaces and incremental patches on one connection.
    pub surface_revision: u64,
    pub frame: FrameData,
    pub panes: Vec<PaneSurfacePane>,
    pub splits: Vec<PaneSurfaceSplit>,
    pub graphics: SurfaceGraphicsScene,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfacePatchRow {
    /// Origin-relative surface column where this changed cell span starts.
    pub x: u16,
    /// Origin-relative surface row.
    pub y: u16,
    pub cells: Vec<CellData>,
}

/// Incremental terminal-cell update against one committed complete pane surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSurfacePatch {
    pub boot_id: String,
    pub projection_revision: u64,
    pub base_surface_revision: u64,
    pub surface_revision: u64,
    pub rows: Vec<PaneSurfacePatchRow>,
    /// Updated metadata for panes whose terminal content changed.
    pub panes: Vec<PaneSurfacePane>,
    /// Final cursor relative to the pane surface.
    pub cursor: Option<CursorState>,
}
