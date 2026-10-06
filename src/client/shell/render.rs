use super::*;

#[path = "../shell/overlays.rs"]
mod overlays;
#[path = "../shell/sidebar.rs"]
pub(in crate::client::shell) mod sidebar;
#[path = "../shell/tabs.rs"]
mod tabs;

pub(super) use super::aggregate_navigation::navigator_rows as client_navigator_rows;
pub(super) use overlays::{render_client_overlay, render_context_menu};
pub(super) use sidebar::workspace_entries;
pub(super) use tabs::tab_bar_status_width;

pub(super) fn render_mode_bar(
    buffer: &mut Buffer,
    pane_area: Rect,
    mode: ClientShellMode,
    copy_mode: Option<&ClientCopyModeState>,
    endpoint_error: Option<&str>,
    keybinds: &LiveKeybindConfig,
    palette: &Palette,
) -> Option<Rect> {
    if (mode == ClientShellMode::Terminal && endpoint_error.is_none()) || pane_area.is_empty() {
        return None;
    }

    let bar = Rect::new(
        pane_area.x,
        pane_area.y + pane_area.height.saturating_sub(1),
        pane_area.width,
        1,
    );
    let base = Style::default().fg(palette.overlay0).bg(palette.panel_bg);
    for x in bar.x..bar.x + bar.width {
        buffer[(x, bar.y)].set_symbol(" ").set_style(base);
    }

    let key = Style::default()
        .fg(palette.accent)
        .bg(palette.panel_bg)
        .add_modifier(Modifier::BOLD);
    let mode_style = Style::default()
        .fg(match palette.panel_bg {
            ratatui::style::Color::Reset => palette.surface_dim,
            color => color,
        })
        .bg(if mode == ClientShellMode::Resize {
            palette.mauve
        } else {
            palette.accent
        })
        .add_modifier(Modifier::BOLD);
    let prefix = crate::config::format_key_combo(keybinds.prefix);
    let prefix_rhs = |bindings: &crate::config::ActionKeybinds| {
        bindings
            .prefix_rhs_label()
            .unwrap_or_else(|| "unset".to_owned())
    };

    let mut segments = Vec::<(String, Style)>::new();
    if let Some(error) = endpoint_error {
        segments.extend([
            (" ERROR ".to_owned(), mode_style),
            (format!(" {error}"), base),
        ]);
    } else {
        match mode {
            ClientShellMode::Prefix => {
                segments.extend([
                    (" PREFIX ".to_owned(), mode_style),
                    (" ".to_owned(), base),
                    ("esc".to_owned(), key),
                    (" cancel  ".to_owned(), base),
                    (prefix, key),
                    (" send prefix  ".to_owned(), base),
                    (prefix_rhs(&keybinds.keybinds.workspace_picker), key),
                    (" workspace nav  ".to_owned(), base),
                    (prefix_rhs(&keybinds.keybinds.help), key),
                    (" keybinds".to_owned(), base),
                ]);
            }
            ClientShellMode::Navigate => {
                segments.extend([
                    (" NAVIGATE ".to_owned(), mode_style),
                    (" esc back  ".to_owned(), base),
                    ("↑/↓".to_owned(), key),
                    (" workspace  ".to_owned(), base),
                    ("tab".to_owned(), key),
                    (" pane  ".to_owned(), base),
                    (prefix_rhs(&keybinds.keybinds.help), key),
                    (" keybinds".to_owned(), base),
                ]);
            }
            ClientShellMode::Resize => {
                segments.extend([
                    (" RESIZE ".to_owned(), mode_style),
                    ("  ".to_owned(), base),
                    ("h/l".to_owned(), key),
                    (" width  ".to_owned(), base),
                    ("j/k".to_owned(), key),
                    (" height  ".to_owned(), base),
                    ("esc".to_owned(), key),
                    (" done".to_owned(), base),
                ]);
            }
            ClientShellMode::Copy => {
                let copy_mode = copy_mode?;
                if let Some(prompt) = copy_mode.search_prompt.as_ref() {
                    let marker = match prompt.direction {
                        crate::api::schema::PaneCopySearchDirection::Forward => "/",
                        crate::api::schema::PaneCopySearchDirection::Backward => "?",
                    };
                    segments.extend([
                        (" COPY ".to_owned(), mode_style),
                        (" ".to_owned(), base),
                        (marker.to_owned(), key),
                        (
                            prompt.query.clone(),
                            Style::default().fg(palette.text).bg(palette.panel_bg),
                        ),
                        ("█".to_owned(), key),
                        ("  enter search  esc cancel".to_owned(), base),
                    ]);
                } else {
                    let select = if copy_mode.selection.is_some() {
                        "selecting"
                    } else {
                        "select"
                    };
                    let match_status = copy_mode
                        .search_current_global
                        .map(|current| format!(" {}/{}", current + 1, copy_mode.search_total))
                        .or_else(|| (!copy_mode.search_query.is_empty()).then(|| " 0/0".to_owned()))
                        .unwrap_or_default();
                    let (exit_keys, exit_label) =
                        if copy_mode.search_query.is_empty() && copy_mode.selection.is_none() {
                            ("q/esc", " exit")
                        } else {
                            ("esc", " clear  q exit")
                        };
                    segments.extend([
                        (" COPY ".to_owned(), mode_style),
                        (" ".to_owned(), base),
                        ("h/j/k/l w/b/e { }".to_owned(), key),
                        (" move  ".to_owned(), base),
                        ("/ ?".to_owned(), key),
                        (" search  ".to_owned(), base),
                        ("n/N".to_owned(), key),
                        (format!(" repeat{match_status}  "), base),
                        ("v/space".to_owned(), key),
                        (format!(" {select}  "), base),
                        ("y/enter".to_owned(), key),
                        (" copy  ".to_owned(), base),
                        (exit_keys.to_owned(), key),
                        (exit_label.to_owned(), base),
                    ]);
                }
            }
            ClientShellMode::Terminal => unreachable!(),
        }
    }

    let mut x = bar.x;
    let end = bar.x + bar.width;
    for (text, style) in segments {
        if x >= end {
            break;
        }
        let remaining = end - x;
        buffer.set_stringn(x, bar.y, &text, usize::from(remaining), style);
        x = x.saturating_add(
            u16::try_from(UnicodeWidthStr::width(text.as_str()))
                .unwrap_or(u16::MAX)
                .min(remaining),
        );
    }
    Some(bar)
}

pub(super) fn put_right_text(buffer: &mut Buffer, area: Rect, y: u16, text: &str, style: Style) {
    let width = display_width(text).min(area.width);
    put_text(
        buffer,
        area.right().saturating_sub(width),
        y,
        width,
        text,
        style,
    );
}

pub(super) fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    if width == 0 || y >= buffer.area.bottom() || x >= buffer.area.right() {
        return;
    }
    buffer.set_stringn(x, y, text, width as usize, style);
}

pub(super) fn display_width(text: &str) -> u16 {
    UnicodeWidthStr::width(text).min(u16::MAX as usize) as u16
}
