pub(crate) mod apc;
pub(crate) mod placement;

#[path = "../../server/rendering/images.rs"]
pub(crate) mod surface;

pub(crate) const HEADLESS_GRAPHICS_TRANSACTION_BUDGET: usize =
    crate::protocol::MAX_GRAPHICS_FRAME_SIZE - crate::protocol::MAX_FRAME_SIZE;
pub(crate) use crate::client::host_terminal::kitty::{
    collect_visible_placements, encode_graphics_update_incremental, image_transfer_estimated_size,
    is_enabled, set_enabled, HostGraphicsCache,
};
#[cfg(test)]
use crate::client::host_terminal::kitty::{drain_graphics_updates, terminal_image_needs_data};
#[cfg(test)]
use crate::ghostty::{KittyImageDescriptor, KittyImageFormat, KittyImagePlacement};
#[cfg(test)]
use crate::layout::PaneId;
pub(crate) use apc::{encode_delete_image, encode_delete_placement};
#[cfg(test)]
use placement::host_placement_id;
pub(crate) use placement::{
    clipped_placement, HostCellSize, HostPlacement, HostSourceKey, ImageSignature,
};
#[cfg(test)]
use ratatui::layout::Rect;
#[cfg(test)]
use std::collections::hash_map::DefaultHasher;
#[cfg(test)]
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::hash::{Hash, Hasher};

#[cfg(test)]
#[path = "tests/placement_test.rs"]
mod tests;
