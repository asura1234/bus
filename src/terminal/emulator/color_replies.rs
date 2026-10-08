use bytes::Bytes;
use tracing::debug;

use super::controls::osc::{
    restore_host_terminal_theme_if_needed, write_host_terminal_theme_selective, DefaultColorEvent,
    DefaultColorQuery, OscTerminator,
};
use super::{GhosttyPaneCore, GhosttyPaneTerminal, PaneTerminal};
use crate::layout::PaneId;

impl PaneTerminal {
    pub fn apply_host_terminal_theme(&self, theme: crate::terminal_theme::TerminalTheme) {
        self.ghostty.apply_host_terminal_theme(theme);
    }

    pub fn apply_host_terminal_appearance(
        &self,
        appearance: Option<crate::terminal_theme::HostAppearance>,
    ) -> Option<Bytes> {
        self.ghostty.apply_host_terminal_appearance(appearance)
    }

    pub fn has_transient_default_color_override(&self) -> bool {
        self.ghostty.has_transient_default_color_override()
    }

    pub fn maybe_restore_host_terminal_theme(&self, pane_id: PaneId, shell_pid: u32) -> bool {
        self.ghostty
            .maybe_restore_host_terminal_theme(pane_id, shell_pid)
    }
}

impl GhosttyPaneTerminal {
    pub fn apply_host_terminal_theme(&self, theme: crate::terminal_theme::TerminalTheme) {
        if let Ok(mut core) = self.core.lock() {
            let foreground_unowned = !core.child_default_foreground_changed;
            let background_unowned = !core.child_default_background_changed;
            core.host_terminal_theme = theme;
            if foreground_unowned && background_unowned {
                core.transient_default_color_owner_pgid = None;
            }

            let mut palette = crate::ghostty::default_palette();
            for (index, color) in theme.palette.iter().enumerate() {
                if let Some(color) = color {
                    palette[index] = crate::ghostty::RgbColor {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                    };
                }
            }
            if let Err(err) = core.terminal.set_default_palette(&palette) {
                debug!(err = %err, "failed to apply host terminal palette");
            }

            write_host_terminal_theme_selective(
                &mut core.terminal,
                theme,
                foreground_unowned,
                background_unowned,
            );
        }
    }

    pub fn apply_host_terminal_appearance(
        &self,
        appearance: Option<crate::terminal_theme::HostAppearance>,
    ) -> Option<Bytes> {
        let mut core = self.core.lock().ok()?;
        let color_scheme = appearance.map(|appearance| match appearance {
            crate::terminal_theme::HostAppearance::Dark => crate::ghostty::ColorScheme::Dark,
            crate::terminal_theme::HostAppearance::Light => crate::ghostty::ColorScheme::Light,
        });
        let previous = core.terminal.set_color_scheme(color_scheme);

        let transitioned = matches!(
            (previous, color_scheme),
            (Some(previous), Some(current)) if previous != current
        );
        if !transitioned
            || !core
                .terminal
                .mode_get(crate::ghostty::MODE_COLOR_SCHEME_REPORT)
                .unwrap_or(false)
        {
            return None;
        }
        // A transition above requires a current color scheme, derived only
        // from a present appearance. Bind that invariant without unwrapping.
        appearance.map(|appearance| Bytes::from_static(appearance.color_scheme_report()))
    }

    pub fn has_transient_default_color_override(&self) -> bool {
        self.core
            .lock()
            .map(|core| core.transient_default_color_owner_pgid.is_some())
            .unwrap_or(false)
    }

    pub fn maybe_restore_host_terminal_theme(&self, pane_id: PaneId, shell_pid: u32) -> bool {
        {
            let Ok(core) = self.core.lock() else {
                return false;
            };
            if !should_probe_host_terminal_theme_restore(&core) {
                return false;
            }
        }

        let foreground_job = crate::detect::foreground_job(shell_pid);
        let Ok(mut core) = self.core.lock() else {
            return false;
        };

        let alternate_screen = core
            .terminal
            .active_screen()
            .map(|screen| screen == crate::ghostty::ActiveScreen::Alternate)
            .unwrap_or(false);
        restore_host_terminal_theme_if_needed(
            &mut core,
            pane_id,
            shell_pid,
            alternate_screen,
            foreground_job.as_ref(),
        )
    }
}

pub(super) fn remove_last_matching_libghostty_color_reply(
    responses: &mut Vec<Bytes>,
    event: DefaultColorEvent,
) {
    if let Some(index) = responses
        .iter()
        .rposition(|response| is_matching_libghostty_color_reply(response, event))
    {
        responses.remove(index);
    }
}

fn is_matching_libghostty_color_reply(response: &Bytes, event: DefaultColorEvent) -> bool {
    let prefix = match event {
        DefaultColorEvent::Query(query) => format!("\x1b]{};rgb:", query.osc_number()),
        DefaultColorEvent::PaletteQuery(index) => format!("\x1b]4;{index};rgb:"),
        DefaultColorEvent::Set(_) | DefaultColorEvent::Reset(_) => return false,
    };
    response.starts_with(prefix.as_bytes())
        && (response.ends_with(b"\x07") || response.ends_with(b"\x1b\\"))
}

pub(super) fn respond_to_default_color_event(
    core: &mut GhosttyPaneCore,
    event: DefaultColorEvent,
    terminator: OscTerminator,
) -> Option<Bytes> {
    match event {
        DefaultColorEvent::Query(query) => {
            default_color_event_response(core, event, terminator, query.osc_number().to_string())
        }
        DefaultColorEvent::PaletteQuery(index) => {
            default_color_event_response(core, event, terminator, format!("4;{index}"))
        }
        DefaultColorEvent::Set(query) => {
            mark_child_default_color_changed(core, query, true);
            None
        }
        DefaultColorEvent::Reset(query) => {
            mark_child_default_color_changed(core, query, false);
            apply_cached_host_default_color(core, query);
            None
        }
    }
}

fn default_color_event_response(
    core: &mut GhosttyPaneCore,
    event: DefaultColorEvent,
    terminator: OscTerminator,
    command: String,
) -> Option<Bytes> {
    let color = default_color_event_color(core, event)?;
    Some(osc_rgb_response(&command, color, terminator))
}

pub(super) fn default_color_event_color(
    core: &mut GhosttyPaneCore,
    event: DefaultColorEvent,
) -> Option<crate::ghostty::RgbColor> {
    match event {
        DefaultColorEvent::Query(query) => default_color_query_color(query, core),
        DefaultColorEvent::PaletteQuery(index) => palette_color_query_color(index, core),
        DefaultColorEvent::Set(_) | DefaultColorEvent::Reset(_) => None,
    }
}

/// Answers an OSC 10/11/12 query. A child that asks the terminal for its default
/// colors can block until it is answered, and libghostty only reports a slot it
/// was explicitly told about, so every query has to resolve to a color here: the
/// host terminal theme when Bus knows it and the child has not overridden that
/// slot, then the value the child itself installed, and finally the color Bus
/// actually paints the pane with.
fn default_color_query_color(
    query: DefaultColorQuery,
    core: &mut GhosttyPaneCore,
) -> Option<crate::ghostty::RgbColor> {
    match query {
        DefaultColorQuery::Foreground => {
            if !core.child_default_foreground_changed {
                if let Some(color) = core.host_terminal_theme.foreground {
                    return Some(host_theme_color_to_ghostty(color));
                }
            }
            if let Some(color) = core.terminal.effective_foreground_color().ok().flatten() {
                return Some(color);
            }
            rendered_colors(core).map(|colors| colors.foreground)
        }
        DefaultColorQuery::Background => {
            if !core.child_default_background_changed {
                if let Some(color) = core.host_terminal_theme.background {
                    return Some(host_theme_color_to_ghostty(color));
                }
            }
            if let Some(color) = core.terminal.effective_background_color().ok().flatten() {
                return Some(color);
            }
            rendered_colors(core).map(|colors| colors.background)
        }
        DefaultColorQuery::Cursor => cursor_color_query_color(core),
    }
}

fn cursor_color_query_color(core: &mut GhosttyPaneCore) -> Option<crate::ghostty::RgbColor> {
    if let Some(color) = core.terminal.effective_cursor_color().ok().flatten() {
        return Some(color);
    }
    if !core.child_default_foreground_changed {
        if let Some(color) = core.host_terminal_theme.foreground {
            return Some(host_theme_color_to_ghostty(color));
        }
    }
    if let Some(color) = core.terminal.effective_foreground_color().ok().flatten() {
        return Some(color);
    }
    rendered_colors(core).map(|colors| colors.foreground)
}

fn palette_color_query_color(
    index: u8,
    core: &mut GhosttyPaneCore,
) -> Option<crate::ghostty::RgbColor> {
    rendered_colors(core).map(|colors| colors.palette[usize::from(index)])
}

/// The colors this pane is currently painted with, which is what a child that
/// asks the terminal about its own colors needs to hear.
fn rendered_colors(core: &mut GhosttyPaneCore) -> Option<crate::ghostty::RenderColors> {
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = core;
    render_state.update(terminal).ok()?;
    render_state.colors().ok()
}

fn osc_rgb_response(
    command: &str,
    color: crate::ghostty::RgbColor,
    terminator: OscTerminator,
) -> Bytes {
    let r = u16::from(color.r) * 257;
    let g = u16::from(color.g) * 257;
    let b = u16::from(color.b) * 257;
    let mut response = format!("\x1b]{command};rgb:{r:04x}/{g:04x}/{b:04x}").into_bytes();
    response.extend_from_slice(terminator.as_bytes());
    Bytes::from(response)
}

fn host_theme_color_to_ghostty(color: crate::terminal_theme::RgbColor) -> crate::ghostty::RgbColor {
    crate::ghostty::RgbColor {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

fn apply_cached_host_default_color(core: &mut GhosttyPaneCore, query: DefaultColorQuery) {
    write_host_terminal_theme_selective(
        &mut core.terminal,
        core.host_terminal_theme,
        matches!(query, DefaultColorQuery::Foreground),
        matches!(query, DefaultColorQuery::Background),
    );
}

fn mark_child_default_color_changed(
    core: &mut GhosttyPaneCore,
    query: DefaultColorQuery,
    changed: bool,
) {
    match query {
        DefaultColorQuery::Foreground => core.child_default_foreground_changed = changed,
        DefaultColorQuery::Background => core.child_default_background_changed = changed,
        DefaultColorQuery::Cursor => {}
    }
}

pub(super) fn should_probe_host_terminal_theme_restore(core: &GhosttyPaneCore) -> bool {
    if core.transient_default_color_owner_pgid.is_none() || core.host_terminal_theme.is_empty() {
        return false;
    }

    !core
        .terminal
        .active_screen()
        .map(|screen| screen == crate::ghostty::ActiveScreen::Alternate)
        .unwrap_or(false)
}
