//! Built-binary callback and isolated CLI-path checks.

#[path = "../support/fixture_dir_test.rs"]
mod utils;
use utils::test_temp as fixture_dirs;

#[path = "callbacks_test.rs"]
mod callbacks;
#[path = "help_test.rs"]
mod help;
#[path = "../../src/utils/tests/live_bus_env_test.rs"]
mod live_bus_env;
#[path = "paths_test.rs"]
mod paths;
#[cfg(unix)]
#[path = "sender_identity_test.rs"]
mod sender_identity;
#[path = "variant_test.rs"]
mod variant;
