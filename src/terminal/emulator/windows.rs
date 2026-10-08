use super::{current_cursor_state, GhosttyPaneCore};

#[cfg(windows)]
pub(super) fn windows_powershell_current_prompt_cwd(
    core: &mut GhosttyPaneCore,
) -> Option<std::path::PathBuf> {
    if core.terminal.active_screen().ok()? != crate::ghostty::ActiveScreen::Primary {
        return None;
    }
    let cursor = current_cursor_state(core)?;
    let rows = core.terminal.rows().ok()?;
    let cols = core.terminal.cols().ok()?;
    if rows == 0 || cols == 0 || cursor.y >= rows {
        return None;
    }
    let total_rows = core.terminal.total_rows().ok()?;
    let viewport_start = total_rows.saturating_sub(usize::from(rows));
    let cursor_row = viewport_start + usize::from(cursor.y);
    if core
        .terminal
        .read_text_screen(
            (0, cursor_row as u32),
            (cols.saturating_sub(1), cursor_row as u32),
            false,
        )
        .ok()?
        .trim()
        .is_empty()
    {
        return None;
    }
    let text = core
        .terminal
        .read_text_screen(
            (0, viewport_start as u32),
            (cols.saturating_sub(1), cursor_row as u32),
            false,
        )
        .ok()?;
    windows_powershell_prompt_cwd(&text)
}

#[cfg(windows)]
pub(super) fn windows_powershell_prompt_cwd(text: &str) -> Option<std::path::PathBuf> {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(windows_powershell_prompt_line_cwd)
}

#[cfg(windows)]
fn windows_powershell_prompt_line_cwd(line: &str) -> Option<std::path::PathBuf> {
    let line = line.trim_end();
    let rest = line.strip_prefix("PS ")?;
    let marker = rest.find('>')?;
    let mut raw_cwd = rest[..marker].trim_end();
    let suffix = rest[marker..].trim_end();
    if !suffix.chars().all(|ch| ch == '>') {
        return None;
    }
    if let Some((_, filesystem_path)) = raw_cwd.rsplit_once("::") {
        raw_cwd = filesystem_path;
    }
    let cwd = std::path::PathBuf::from(raw_cwd);
    (cwd.is_absolute() && cwd.is_dir()).then_some(cwd)
}
