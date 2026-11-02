//! Resolve Kimi's ambiguous activity chrome against the native main turn.
use crate::agents::manifest::{self, KimiTurnMarker};
use crate::agents::providers::kimi::activity;
use crate::agents::providers::kimi::wire::{MainTurn, MainTurnCache};
use crate::agents::{AgentDetection, AgentState};
use crate::terminal::emulator::PaneTerminal;

/// Resolve command/tip markers and an empty composer against the native main turn.
/// `None` means the remaining screen is a viewer.
pub(super) fn classify_command_screen(
    terminal: &PaneTerminal,
    home: Option<&std::path::Path>,
    cache: &mut MainTurnCache,
    content: &str,
    screen_detection: AgentDetection,
) -> Option<AgentDetection> {
    let marker = manifest::kimi_turn_marker(content);
    if marker.is_none() && !screen_detection.visible_idle {
        return Some(screen_detection);
    }
    let turn = activity::refresh_running_command_turn(home, cache);
    let Some(marker) = marker else {
        let mut detection = screen_detection;
        if turn == Some(MainTurn::Open) {
            detection.state = AgentState::Working;
            detection.visible_idle = false;
        }
        return Some(detection);
    };
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
