//! Resolve Kimi's ambiguous activity chrome against the native main turn.
use crate::agents::manifest::{self, KimiTurnMarker};
use crate::agents::providers::kimi::activity;
use crate::agents::providers::kimi::wire::{MainTurn, MainTurnCache};
use crate::agents::AgentDetection;
use crate::terminal::emulator::PaneTerminal;

/// Resolve ambiguous command/tip markers against native main-turn evidence.
/// `None` means the remaining screen is a viewer.
pub(super) fn classify_command_screen(
    terminal: &PaneTerminal,
    home: Option<&std::path::Path>,
    cache: &mut MainTurnCache,
    content: &str,
    screen_detection: AgentDetection,
) -> Option<AgentDetection> {
    let Some(marker) = manifest::kimi_turn_marker(content) else {
        return Some(screen_detection);
    };
    let turn = activity::refresh_running_command_turn(home, cache);
    let live = match marker {
        KimiTurnMarker::Tip => turn != Some(MainTurn::Settled),
        KimiTurnMarker::Command => {
            let has_command_suffix = activity::running_command_has_command_suffix(content);
            let ansi = turn.is_none().then(|| terminal.visible_ansi());
            let line_is_bold = ansi
                .as_deref()
                .and_then(activity::running_command_line_is_bold);
            activity::running_command_text_is_live(turn, line_is_bold, has_command_suffix)
        }
    };
    if live {
        return Some(screen_detection);
    }
    let adjusted = manifest::kimi_screen_without_running_command(content);
    (!adjusted.skip_state_update).then_some(adjusted)
}
