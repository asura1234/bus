mod cache;
mod visible;

#[cfg(test)]
pub(crate) use cache::drain_graphics_updates;
pub(crate) use cache::{
    encode_graphics_update_incremental, image_transfer_estimated_size, is_enabled, set_enabled,
    HostGraphicsCache,
};
pub(crate) use visible::collect_visible_placements;
#[cfg(test)]
pub(crate) use visible::terminal_image_needs_data;
