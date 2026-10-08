// Host-side VT observation of the built client's PTY output.
// Only the generated C ABI is shared; no Bus component or test module is loaded.
#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all
)]
mod ffi {
    include!("../../src/terminal/vt/ffi.rs");
}

use std::{mem, ptr};

pub(super) struct HostScreen {
    raw: ffi::GhosttyTerminal,
    cols: u16,
    rows: u16,
}

impl HostScreen {
    pub(super) fn new(cols: u16, rows: u16) -> Self {
        let mut raw = ptr::null_mut();
        let options = ffi::GhosttyTerminalOptions {
            cols,
            rows,
            max_scrollback: 0,
        };
        let result = unsafe { ffi::ghostty_terminal_new(ptr::null(), &mut raw, options) };
        assert_eq!(result, ffi::GhosttyResult_GHOSTTY_SUCCESS);
        Self { raw, cols, rows }
    }

    pub(super) fn write(&mut self, bytes: &[u8]) {
        unsafe { ffi::ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len()) };
    }

    fn cell(&self, x: u16, y: u32) -> char {
        let point = ffi::GhosttyPoint {
            tag: ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN,
            value: ffi::GhosttyPointValue {
                coordinate: ffi::GhosttyPointCoordinate { x, y },
            },
        };
        let mut reference = ffi::GhosttyGridRef {
            size: mem::size_of::<ffi::GhosttyGridRef>(),
            ..Default::default()
        };
        assert_eq!(
            unsafe { ffi::ghostty_terminal_grid_ref(self.raw, point, &mut reference) },
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        let mut required = 0;
        let result = unsafe {
            ffi::ghostty_grid_ref_graphemes(&reference, ptr::null_mut(), 0, &mut required)
        };
        assert!(
            result == ffi::GhosttyResult_GHOSTTY_SUCCESS
                || result == ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE
        );
        if required == 0 {
            return ' ';
        }
        let mut graphemes = vec![0; required];
        assert_eq!(
            unsafe {
                ffi::ghostty_grid_ref_graphemes(
                    &reference,
                    graphemes.as_mut_ptr(),
                    graphemes.len(),
                    &mut required,
                )
            },
            ffi::GhosttyResult_GHOSTTY_SUCCESS
        );
        char::from_u32(graphemes[0]).unwrap_or(' ')
    }

    pub(super) fn margin(&self) -> String {
        (0..u32::from(self.rows))
            .map(|row| self.cell(self.cols - 1, row))
            .collect()
    }

    pub(super) fn text(&self) -> String {
        (0..u32::from(self.rows))
            .flat_map(|row| (0..self.cols).map(move |col| self.cell(col, row)))
            .collect()
    }
}

impl Drop for HostScreen {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_terminal_free(self.raw) };
    }
}
