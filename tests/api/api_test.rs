#![cfg(unix)]

#[path = "../support/fixture_dir_test.rs"]
mod fixture_dirs;

#[path = "../../src/utils/tests/live_bus_env_test.rs"]
mod live_bus_env;
#[path = "../support/process_test.rs"]
pub mod support;

use std::fs;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::thread;
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;
use support::*;

#[path = "agents_test.rs"]
mod agents;
#[path = "events_test.rs"]
mod events;
#[cfg(target_os = "linux")]
#[path = "panes_test.rs"]
mod panes;
#[path = "server_test.rs"]
mod server;
#[path = "workspaces_tabs_test.rs"]
mod workspaces_tabs;
