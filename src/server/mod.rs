pub(crate) mod api;
pub(crate) mod app;
mod app_loop;
pub(crate) mod app_state;
pub(crate) mod clients;
pub mod main_loop;
pub(crate) mod notifications;
pub(crate) mod persistence;
pub(crate) mod rendering;
pub(crate) mod shutdown;
pub(crate) mod startup;
pub(crate) mod terminals;
pub(crate) mod workspaces;
pub(crate) use crate::utils::socket_paths;
pub use main_loop as headless;
#[cfg(test)]
#[path = "tests/render_scale.rs"]
mod render_scale_benchmark;
#[cfg(test)]
mod tests;
