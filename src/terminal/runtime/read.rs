use crate::layout::PaneId;
use crate::terminal::emulator::PaneTerminal;
use crate::terminal::events::AppEvent;
use crate::terminal::pty::actor::PtyReadResult;
use crate::terminal::runtime::compression::TerminalCompressionWake;
use crate::terminal::runtime::detection_policy::observe_detection_content_change;
#[cfg(unix)]
use crate::terminal::runtime::detection_process::absolute_process_cwd;
#[cfg(unix)]
use crate::terminal::runtime::detection_process::foreground_member_cwd_different_from_shell;
#[cfg(unix)]
use crate::terminal::runtime::detection_process::usable_process_cwd;
use crate::terminal::runtime::AgentDetection;
use crate::terminal::runtime::ScrollMetrics;
use crate::terminal::runtime::TerminalCursorState;
use crate::terminal::runtime::TerminalDirtyPatchOutcome;
use crate::terminal::runtime::TerminalReadSnapshot;
use crate::terminal::runtime::TerminalRuntime;
use crate::utils::render::signal::RenderSignal;
use bytes::Bytes;
use ratatui::layout::Rect;
use ratatui::Frame;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tracing::warn;

pub(super) fn usable_reported_cwd(cwd: std::path::PathBuf) -> Option<std::path::PathBuf> {
    (cwd.is_absolute() && cwd.is_dir()).then_some(cwd)
}

pub(super) fn publish_terminal_bells(pane_id: PaneId, count: u16, events: &mpsc::Sender<AppEvent>) {
    if count == 0 {
        return;
    }
    if let Err(err) = events.try_send(AppEvent::TerminalBell { pane_id, count }) {
        warn!(
            pane = pane_id.raw(),
            count,
            err = %err,
            "failed to queue terminal bell"
        );
    }
}

pub(super) fn publish_reported_cwd(
    pane_id: PaneId,
    cwd: std::path::PathBuf,
    reported_cwd: &Arc<Mutex<Option<std::path::PathBuf>>>,
    events: &mpsc::Sender<AppEvent>,
) {
    let Some(cwd) = usable_reported_cwd(cwd) else {
        return;
    };
    if let Ok(mut current) = reported_cwd.lock() {
        if current.as_ref() == Some(&cwd) {
            return;
        }
        *current = Some(cwd.clone());
    }
    if let Err(err) = events.try_send(AppEvent::TerminalCwdReported { pane_id, cwd }) {
        warn!(
            pane = pane_id.raw(),
            err = %err,
            "failed to send terminal cwd report"
        );
    }
}

pub(super) struct PtyReadContext {
    pub(super) pane_id: PaneId,
    pub(super) terminal: Arc<PaneTerminal>,
    pub(super) response_writer: mpsc::Sender<Bytes>,
    pub(super) render_notify: Arc<Notify>,
    pub(super) render_dirty: Arc<RenderSignal>,
    pub(super) content_seq: Arc<AtomicU64>,
    pub(super) content_write_lock: Arc<Mutex<()>>,
    pub(super) detection_content_seq: Arc<AtomicU64>,
    pub(super) child_pid: Arc<AtomicU32>,
    pub(super) events: mpsc::Sender<AppEvent>,
    pub(super) reported_cwd: Arc<Mutex<Option<std::path::PathBuf>>>,
    pub(super) compression_wake: TerminalCompressionWake,
    pub(super) rt: tokio::runtime::Handle,
    pub(super) agent_detection: AgentDetection,
}

type PtyReadCallback = Box<dyn FnMut(&[u8]) -> PtyReadResult + Send>;

pub(super) fn pty_read_callback(context: PtyReadContext) -> PtyReadCallback {
    let PtyReadContext {
        pane_id,
        terminal,
        response_writer,
        render_notify,
        render_dirty,
        content_seq,
        content_write_lock,
        detection_content_seq,
        child_pid,
        events,
        reported_cwd,
        compression_wake,
        rt,
        agent_detection,
    } = context;
    Box::new(move |bytes: &[u8]| {
        let _content_write_guard = match content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        content_seq.fetch_add(1, Ordering::AcqRel);
        let shell_pid = child_pid.load(Ordering::Acquire);
        let result = terminal.process_pty_bytes(
            pane_id,
            shell_pid,
            bytes,
            &response_writer,
            super::detection_process::foreground_job,
        );
        content_seq.fetch_add(1, Ordering::Release);
        drop(_content_write_guard);
        compression_wake.wake();
        publish_terminal_bells(pane_id, result.terminal_bells, &events);
        if agent_detection == AgentDetection::Enabled {
            observe_detection_content_change(bytes, &detection_content_seq);
        }
        let title_requested =
            result.terminal_title_changed && render_dirty.request_terminal_title(pane_id);
        let render_requested = result.request_render && render_dirty.request_pty(pane_id);
        if title_requested || render_requested {
            render_notify.notify_one();
        }
        if let Some(delay) = result.render_delay {
            let render_notify = render_notify.clone();
            let render_dirty = render_dirty.clone();
            rt.spawn(async move {
                tokio::time::sleep(delay).await;
                if render_dirty.request_pty(pane_id) {
                    render_notify.notify_one();
                }
            });
        }
        if let Some(cwd) = result.reported_cwd.clone() {
            publish_reported_cwd(pane_id, cwd, &reported_cwd, &events);
        }
        for content in result.clipboard_writes {
            if let Err(err) = events.try_send(AppEvent::ClipboardWrite { content }) {
                warn!(
                    pane = pane_id.raw(),
                    err = %err,
                    "failed to send OSC 52 clipboard write"
                );
            }
        }
        PtyReadResult {
            terminal_responses: result.terminal_responses,
        }
    })
}

impl TerminalRuntime {
    /// Scroll up by N lines (into scrollback history).
    pub fn scroll_up(&self, lines: usize) {
        self.terminal.scroll_up(lines);
        self.compression.wake();
    }

    /// Scroll down by N lines (toward live output).
    pub fn scroll_down(&self, lines: usize) {
        self.terminal.scroll_down(lines);
        self.compression.wake();
    }

    /// Reset scroll to live view (offset = 0).
    pub fn scroll_reset(&self) {
        self.terminal.scroll_reset();
        self.compression.wake();
    }

    /// Set scrollback offset measured from the live bottom of the terminal.
    pub fn set_scroll_offset_from_bottom(&self, lines: usize) {
        self.terminal.set_scroll_offset_from_bottom(lines);
        self.compression.wake();
    }

    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        self.terminal.scroll_metrics()
    }

    pub(crate) fn search_text_window(
        &self,
        query: &str,
        case_sensitive: bool,
        direction: crate::terminal::runtime::TerminalSearchDirection,
        cursor: crate::terminal::runtime::TerminalTextPoint,
        previous: Option<(
            crate::terminal::runtime::TerminalTextPoint,
            crate::terminal::runtime::TerminalTextPoint,
        )>,
        limit: usize,
    ) -> crate::terminal::runtime::TerminalSearchWindow {
        let result = self.terminal.search_text_window(
            query,
            case_sensitive,
            direction,
            cursor,
            previous,
            limit,
        );
        self.compression.wake();
        result
    }

    pub(crate) fn word_motion_target(
        &self,
        row: u32,
        col: u16,
        motion: crate::terminal::runtime::TerminalWordMotion,
    ) -> Option<crate::terminal::runtime::TerminalTextPoint> {
        let result = self.terminal.word_motion_target(row, col, motion);
        self.compression.wake();
        result
    }

    pub(crate) fn terminal_dimensions(&self) -> Option<(u16, u16)> {
        self.terminal.dimensions()
    }

    pub(crate) fn paragraph_motion_target(
        &self,
        row: u32,
        direction: i8,
    ) -> Option<crate::terminal::runtime::TerminalTextPoint> {
        let result = self.terminal.paragraph_motion_target(row, direction);
        self.compression.wake();
        result
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.terminal.bracketed_paste_enabled()
    }

    pub fn focus_reporting_enabled(&self) -> bool {
        self.terminal.focus_reporting_enabled()
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.terminal.mouse_reporting_enabled()
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.terminal.sgr_pixel_mouse_enabled()
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        self.terminal.plain_page_keys_use_host_scrollback()
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.terminal.alternate_screen_active()
    }

    pub fn cursor_state(&self, area: Rect, show_cursor: bool) -> Option<TerminalCursorState> {
        if !show_cursor {
            return None;
        }
        let cursor = self.terminal.cursor_state()?;
        if cursor.x >= area.width || cursor.y >= area.height {
            return None;
        }
        Some(TerminalCursorState {
            x: area.x + cursor.x,
            y: area.y + cursor.y,
            visible: cursor.visible,
            shape: cursor.shape,
        })
    }

    pub fn synchronized_output_active(&self) -> bool {
        self.terminal.synchronized_output_active()
    }

    pub fn visible_text(&self) -> String {
        self.terminal.visible_text()
    }

    /// The visible screen with ANSI styling, as dialog detection reads it.
    pub(crate) fn visible_ansi_snapshot_with_seq(&self) -> Option<(String, u64)> {
        for _ in 0..3 {
            let before = self.content_seq.load(Ordering::Acquire);
            if !before.is_multiple_of(2) {
                continue;
            }
            let text = self.terminal.visible_ansi();
            let after = self.content_seq.load(Ordering::Acquire);
            if before == after {
                return Some((text, after));
            }
        }
        None
    }

    pub(crate) fn visible_text_snapshot_with_dimensions(&self) -> Option<(String, u16, u16, u64)> {
        for _ in 0..3 {
            let before = self.content_seq.load(Ordering::Acquire);
            if !before.is_multiple_of(2) {
                continue;
            }
            let (rows, columns) = self.current_size();
            let text = self.terminal.visible_text();
            let after = self.content_seq.load(Ordering::Acquire);
            if before == after {
                return Some((text, rows, columns, after));
            }
        }
        None
    }

    pub fn visible_ansi(&self) -> String {
        self.terminal.visible_ansi()
    }

    pub fn detection_text(&self) -> String {
        self.terminal.detection_text()
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.terminal.terminal_title()
    }

    pub(crate) fn recent_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_text_snapshot(lines);
        self.compression.wake();
        result
    }

    pub(crate) fn recent_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_ansi_snapshot(lines);
        self.compression.wake();
        result
    }

    pub(crate) fn recent_unwrapped_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_unwrapped_text_snapshot(lines);
        self.compression.wake();
        result
    }

    pub fn recent_unwrapped_ansi(&self, lines: usize) -> String {
        let result = self.terminal.recent_unwrapped_ansi(lines);
        self.compression.wake();
        result
    }

    pub(crate) fn recent_unwrapped_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_unwrapped_ansi_snapshot(lines);
        self.compression.wake();
        result
    }

    pub fn snapshot_history(&self) -> Option<String> {
        let ansi = self.recent_unwrapped_ansi(usize::MAX);
        (!ansi.trim().is_empty()).then_some(ansi)
    }

    pub fn extract_selection(
        &self,
        selection: &crate::utils::text::selection::Selection,
    ) -> Option<String> {
        let result = self.terminal.extract_selection(selection);
        self.compression.wake();
        result
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, show_cursor: bool) {
        self.terminal.render(frame, area, show_cursor);
    }

    pub(crate) fn collect_dirty_patch(
        &self,
        area_width: u16,
        area_height: u16,
    ) -> TerminalDirtyPatchOutcome {
        self.terminal.collect_dirty_patch(area_width, area_height)
    }

    pub fn visible_hyperlinks(&self, area: Rect) -> Vec<((u16, u16), String, String)> {
        self.terminal.visible_hyperlinks(area)
    }

    pub(crate) fn kitty_graphics_may_have_placements(&self) -> bool {
        self.terminal.kitty_graphics_may_have_placements()
    }

    pub fn kitty_image_placements_with_data_filter<F>(
        &self,
        needs_data: F,
    ) -> Vec<crate::terminal::vt::KittyImagePlacement>
    where
        F: FnMut(crate::terminal::vt::KittyImageDescriptor) -> bool,
    {
        self.terminal
            .kitty_image_placements_with_data_filter(needs_data)
    }

    pub(super) fn paste_payload(&self, text: String) -> Bytes {
        let text = crate::platform::prepare_paste_text_for_pty(text);
        let bracketed = self.bracketed_paste_enabled();
        let payload = if bracketed {
            format!("\x1b[200~{text}\x1b[201~")
        } else {
            text
        };
        Bytes::from(payload)
    }

    pub(crate) fn screen_text_snapshot(
        &self,
    ) -> Option<(
        crate::terminal::vt::ActiveScreen,
        crate::terminal::ScreenSnapshot,
    )> {
        let result = self.terminal.screen_text_snapshot();
        self.compression.wake();
        let (screen, cols, rows) = result?;
        Some((screen, crate::terminal::ScreenSnapshot { cols, rows }))
    }

    /// Get the current working directory of the child shell process.
    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        if let Some(cwd) = self
            .reported_cwd
            .lock()
            .ok()
            .and_then(|reported_cwd| reported_cwd.clone())
        {
            return Some(cwd);
        }

        let pid = self.child_pid.load(Ordering::Relaxed);
        crate::platform::process_cwd(pid)
    }

    pub fn child_pid(&self) -> Option<u32> {
        let pid = self.child_pid.load(Ordering::Acquire);
        (pid > 0).then_some(pid)
    }

    pub fn follow_cwd(&self) -> Option<std::path::PathBuf> {
        #[cfg(unix)]
        {
            let leader_cwd = self
                .io
                .foreground_process_group_id()
                .and_then(usable_process_cwd);
            leader_cwd.or_else(|| self.cwd())
        }

        #[cfg(not(unix))]
        {
            self.cwd()
        }
    }

    /// Get the current working directory of the process group controlling the pane PTY.
    pub fn foreground_cwd(&self) -> Option<std::path::PathBuf> {
        #[cfg(unix)]
        {
            let pid = self.child_pid.load(Ordering::Acquire);
            let shell_cwd = absolute_process_cwd(pid);
            let foreground_pgid = self
                .io
                .foreground_process_group_id()
                .or_else(|| crate::platform::foreground_process_group_id(pid));
            let leader_cwd = foreground_pgid.and_then(absolute_process_cwd);

            // The group leader's cwd is authoritative (issue #3270): a helper
            // process that chdirs elsewhere inside the same foreground group
            // must not override it. Scan other members only when the leader's
            // cwd cannot be read at all.
            leader_cwd
                .or_else(|| foreground_member_cwd_different_from_shell(pid, shell_cwd.as_ref()))
        }

        #[cfg(not(unix))]
        {
            None
        }
    }

    #[cfg(test)]
    pub fn recent_unwrapped_text(&self, lines: usize) -> String {
        self.recent_unwrapped_text_snapshot(lines).text
    }

    pub(crate) fn screen_text_snapshot_with_seq(
        &self,
    ) -> Option<(
        crate::terminal::vt::ActiveScreen,
        crate::terminal::ScreenSnapshot,
        u64,
    )> {
        for _ in 0..3 {
            let before = self.content_seq();
            if !before.is_multiple_of(2) {
                continue;
            }
            let (screen, snapshot) = self.screen_text_snapshot()?;
            let after = self.content_seq();
            if before == after {
                return Some((screen, snapshot, after));
            }
        }
        None
    }
}
