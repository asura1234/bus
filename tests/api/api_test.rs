#![cfg(unix)]

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
