//! Built-binary callback and isolated CLI-path checks.

#[path = "../../src/utils/tests/temp_path_test.rs"]
mod fixture_dirs;

#[path = "callbacks_test.rs"]
mod callbacks;
#[path = "help_test.rs"]
mod help;
#[path = "../../src/utils/tests/live_bus_env_test.rs"]
mod live_bus_env;
#[path = "paths_test.rs"]
mod paths;
#[path = "variant_test.rs"]
mod variant;
