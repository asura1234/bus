//! Dev tools: commands that inspect or drive Bus itself. They are compiled
//! only into the dev variant (cargo feature `dev-tools`) and are available
//! only through `bus --dev`.
pub(crate) mod error_fixture;
pub(crate) mod tui;

#[cfg(all(test, unix))]
#[path = "tests/disk_space_test.rs"]
mod disk_space_tests;
