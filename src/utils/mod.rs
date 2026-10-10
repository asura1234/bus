pub mod config;
pub(crate) mod env;
pub(crate) mod home_path;
pub(crate) mod ids;
pub(crate) mod log_events;
pub(crate) mod logging;
pub(crate) mod paths;
pub(crate) mod render;
#[path = "paths/socket.rs"]
pub(crate) mod socket_paths;
pub(crate) mod text;
pub(crate) mod theme;
pub(crate) mod url;
pub(crate) mod version;

#[cfg(test)]
#[path = "config/tests/env_test.rs"]
pub(crate) mod test_env;
#[cfg(test)]
#[path = "tests/temp_path_test.rs"]
pub(crate) mod test_temp;
pub(crate) mod time;
