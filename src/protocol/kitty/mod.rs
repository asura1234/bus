pub(crate) mod apc;
pub(crate) mod placement;
pub(crate) const HEADLESS_GRAPHICS_TRANSACTION_BUDGET: usize =
    crate::protocol::wire::MAX_GRAPHICS_FRAME_SIZE - crate::protocol::wire::MAX_FRAME_SIZE;
pub(crate) use placement::{is_enabled, set_enabled, HostCellSize};
#[cfg(test)]
#[path = "../../client/host_terminal/kitty/tests/placement_test.rs"]
mod tests;
#[cfg(test)]
mod surface {
    mod tests {
        include!("../../client/host_terminal/kitty/tests/scene_test.rs");
    }
}
