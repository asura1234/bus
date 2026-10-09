//! Terminal ownership, callbacks, palette, resizing and mode state.
use super::callbacks::{
    bell_trampoline, clipboard_write_trampoline, color_scheme_trampoline, pwd_changed_trampoline,
    size_trampoline, write_pty_trampoline, TerminalCallbackState,
};
use super::kitty::KittyImageFingerprintEntry;
use super::{ffi, ColorScheme, Error, GhosttyResultExt, RgbColor, TerminalCompressionResult};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::{mem, ptr};

pub struct Terminal {
    pub(super) raw: ffi::GhosttyTerminal,
    #[cfg(windows)]
    pub(super) max_scrollback: usize,
    #[cfg(windows)]
    pub(super) tracked_row: ffi::GhosttyTrackedGridRef,
    pub(super) callback_state: Box<TerminalCallbackState>,
    pub(super) kitty_fingerprints: Mutex<HashMap<u32, KittyImageFingerprintEntry>>,
    pub(super) kitty_empty_generation: Cell<Option<u64>>,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        let options = ffi::GhosttyTerminalOptions {
            cols,
            rows,
            max_scrollback,
        };
        // SAFETY: valid out pointer and options, null allocator means default allocator.
        unsafe {
            ffi::ghostty_terminal_new(ptr::null(), &mut raw, options).into_result()?;
        }

        let mut terminal = Self {
            raw,
            #[cfg(windows)]
            max_scrollback,
            #[cfg(windows)]
            tracked_row: ptr::null_mut(),
            callback_state: Box::new(TerminalCallbackState {
                size_report: ffi::GhosttySizeReportSize {
                    rows,
                    columns: cols,
                    ..Default::default()
                },
                ..Default::default()
            }),
            kitty_fingerprints: Mutex::new(HashMap::new()),
            kitty_empty_generation: Cell::new(None),
        };
        let userdata = (&mut *terminal.callback_state as *mut TerminalCallbackState).cast();
        let glyph_protocol = false;
        unsafe {
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_USERDATA,
                userdata,
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SIZE,
                (size_trampoline as *const ()).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_BELL,
                (bell_trampoline as *const ()).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
                (pwd_changed_trampoline as *const ()).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
                (clipboard_write_trampoline as *const ()).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
                (color_scheme_trampoline as *const ()).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL,
                (&glyph_protocol as *const bool).cast(),
            )
            .into_result()?;
        }
        Ok(terminal)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        // SAFETY: self.raw is a live terminal handle for self's lifetime.
        unsafe {
            ffi::ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len());
        }
    }

    pub(crate) fn compression_activity(&self) -> Result<u64, Error> {
        let mut activity = 0;
        // SAFETY: self.raw is a live terminal handle and activity is a valid out pointer.
        unsafe {
            ffi::ghostty_terminal_compression_activity(self.raw, &mut activity).into_result()?;
        }
        Ok(activity)
    }

    pub(crate) fn compress_incremental(&mut self) -> Result<TerminalCompressionResult, Error> {
        let mut result =
            ffi::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED;
        // SAFETY: self.raw is a live terminal handle and result is a valid out pointer.
        unsafe {
            ffi::ghostty_terminal_compress(
                self.raw,
                ffi::GhosttyTerminalCompressionMode_GHOSTTY_TERMINAL_COMPRESSION_MODE_INCREMENTAL,
                &mut result,
            )
            .into_result()?;
        }
        match result {
            ffi::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED => {
                Ok(TerminalCompressionResult::Unsupported)
            }
            ffi::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_PENDING => {
                Ok(TerminalCompressionResult::Pending)
            }
            ffi::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE => {
                Ok(TerminalCompressionResult::Complete)
            }
            _ => Err(Error(ffi::GhosttyResult_GHOSTTY_INVALID_VALUE)),
        }
    }

    pub fn set_default_palette(&mut self, palette: &[RgbColor; 256]) -> Result<(), Error> {
        let palette = palette.map(|color| ffi::GhosttyColorRgb {
            r: color.r,
            g: color.g,
            b: color.b,
        });
        unsafe {
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                palette.as_ptr().cast(),
            )
            .into_result()
        }
    }

    pub fn default_palette(&self) -> Result<[RgbColor; 256], Error> {
        let mut out = [ffi::GhosttyColorRgb::default(); 256];
        // SAFETY: self.raw is a live terminal handle, and out is exactly the
        // 256-entry array this data kind writes.
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_PALETTE_DEFAULT,
                out.as_mut_ptr().cast(),
            )
            .into_result()?;
        }
        Ok(out.map(Into::into))
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), Error> {
        let size_report = ffi::GhosttySizeReportSize {
            rows,
            columns: cols,
            cell_width: cell_width_px,
            cell_height: cell_height_px,
        };
        // SAFETY: self.raw is valid and sizes are plain values.
        unsafe {
            ffi::ghostty_terminal_resize(
                self.raw,
                cols,
                rows,
                cell_width_px.max(1),
                cell_height_px.max(1),
            )
            .into_result()?;
        }
        self.callback_state.size_report = size_report;
        Ok(())
    }

    pub fn set_write_pty_callback<F>(&mut self, callback: F) -> Result<(), Error>
    where
        F: FnMut(&[u8]) + Send + 'static,
    {
        unsafe {
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                (write_pty_trampoline as *const ()).cast(),
            )
            .into_result()?;
        }
        self.callback_state.write_pty = Some(Box::new(callback));
        Ok(())
    }

    pub fn set_color_scheme(&mut self, color_scheme: Option<ColorScheme>) -> Option<ColorScheme> {
        mem::replace(&mut self.callback_state.color_scheme, color_scheme)
    }

    pub fn take_bell_count(&mut self) -> u16 {
        mem::take(&mut self.callback_state.bell_count)
    }

    pub fn take_pwd_changes(&mut self) -> Vec<Vec<u8>> {
        mem::take(&mut self.callback_state.pwd_changes)
    }

    pub fn take_clipboard_writes(&mut self) -> Vec<Vec<u8>> {
        mem::take(&mut self.callback_state.clipboard_writes)
    }

    pub fn mode_get(&self, mode: u16) -> Result<bool, Error> {
        let mut out = false;
        unsafe { ffi::ghostty_terminal_mode_get(self.raw, mode, &mut out).into_result()? };
        Ok(out)
    }

    #[cfg(test)]
    pub fn mode_set(&mut self, mode: u16, value: bool) -> Result<(), Error> {
        unsafe { ffi::ghostty_terminal_mode_set(self.raw, mode, value).into_result() }
    }

    pub fn kitty_keyboard_flags(&self) -> Result<u8, Error> {
        let mut out = 0u8;
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                (&mut out as *mut u8).cast(),
            )
            .into_result()?;
        }
        Ok(out)
    }

    pub fn mouse_tracking_enabled(&self) -> Result<bool, Error> {
        self.get_bool(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING)
    }

    pub(super) fn raw(&self) -> ffi::GhosttyTerminal {
        self.raw
    }
}

// SAFETY: these opaque handles are only used behind external synchronization in pane runtime.
unsafe impl Send for Terminal {}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: freeing a null or live handle is allowed by the C API.
        unsafe {
            #[cfg(windows)]
            ffi::ghostty_tracked_grid_ref_free(self.tracked_row);
            ffi::ghostty_terminal_free(self.raw);
        }
    }
}
