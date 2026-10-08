pub(crate) mod full;
pub(crate) mod incremental;
pub(crate) mod snapshot;
pub(crate) mod snapshot_graphics;
pub(crate) mod stream;
pub(crate) mod surface;
mod window_title;
// The physical image implementation retains its Kitty parent through S11.
pub(crate) use crate::protocol::kitty::surface as images;
