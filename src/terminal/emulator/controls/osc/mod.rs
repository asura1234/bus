mod agent;
mod collector;
mod cwd;
mod debug;
mod default_colors;
mod scrollback_compat;

pub(in crate::terminal::emulator) use agent::AgentOscStateTracker;
#[cfg(test)]
use agent::AGENT_OSC_MAX_CHARS;
#[cfg(test)]
use collector::OscStreamCollector;
pub(in crate::terminal::emulator) use cwd::parse_reported_cwd;
#[cfg(test)]
use debug::OscDebugEvent;
pub(in crate::terminal::emulator) use debug::OscDebugTracker;
#[cfg(test)]
use default_colors::should_restore_host_terminal_theme;
pub(in crate::terminal::emulator) use default_colors::{
    current_transient_default_color_owner, restore_host_terminal_theme_if_needed,
    write_host_terminal_theme_selective, DefaultColorEvent, DefaultColorEventTracker,
    DefaultColorOscTracker, DefaultColorQuery, DefaultColorTrackedEvent, OscTerminator,
};
pub(in crate::terminal::emulator) use scrollback_compat::{
    contains_scrollback_clear_sequence, maybe_filter_primary_screen_scrollback_clear,
};
#[cfg(test)]
use scrollback_compat::{
    foreground_job_uses_droid_scrollback_compat, strip_scrollback_clear_sequences,
};

#[cfg(test)]
use crate::terminal::emulator::GhosttyPaneCore;
#[cfg(test)]
#[path = "tests/osc_test.rs"]
mod tests;
