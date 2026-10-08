use super::consts::{
    TERMINAL_DATA_COLOR_BACKGROUND, TERMINAL_DATA_COLOR_CURSOR, TERMINAL_DATA_COLOR_FOREGROUND,
};
use super::{
    ffi, ActiveScreen, CellStyle, CellWide, Error, GhosttyResultExt, RgbColor, ScreenTextCell,
    ScreenTextRow, Terminal, TerminalScrollbar,
};
use std::{mem, ptr, slice};

mod rows;
pub use rows::{CellBasicData, RowCellIter, RowCells, RowIter, RowIterator};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormatterFormat {
    Plain,
    Vt,
}

impl FormatterFormat {
    fn as_raw(self) -> ffi::GhosttyFormatterFormat {
        match self {
            Self::Plain => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
            Self::Vt => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_VT,
        }
    }
}

impl Terminal {
    pub fn active_screen(&self) -> Result<ActiveScreen, Error> {
        let mut out = ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                (&mut out as *mut ffi::GhosttyTerminalScreen).cast(),
            )
            .into_result()?;
        }
        Ok(match out {
            ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_PRIMARY => ActiveScreen::Primary,
            ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE => ActiveScreen::Alternate,
            _ => ActiveScreen::Primary,
        })
    }

    pub fn total_rows(&self) -> Result<usize, Error> {
        self.get_usize(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TOTAL_ROWS)
    }

    #[cfg(test)]
    pub fn scrollback_rows(&self) -> Result<usize, Error> {
        self.get_usize(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS)
    }

    #[cfg(windows)]
    pub fn max_scrollback(&self) -> usize {
        self.max_scrollback
    }

    pub fn scrollbar(&self) -> Result<TerminalScrollbar, Error> {
        let mut out = ffi::GhosttyTerminalScrollbar::default();
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                (&mut out as *mut ffi::GhosttyTerminalScrollbar).cast(),
            )
            .into_result()?;
        }
        Ok(TerminalScrollbar {
            total: out.total as usize,
            offset: out.offset as usize,
            len: out.len as usize,
        })
    }

    #[cfg(windows)]
    pub(crate) fn track_row(&mut self, y: u32) -> Option<usize> {
        let mut point = ffi::GhosttyPointCoordinate::default();
        let tag = ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN;
        let result =
            unsafe { ffi::ghostty_tracked_grid_ref_point(self.tracked_row, tag, &mut point) };
        let terminal = self.raw;
        let target = ghostty_viewport_point(0, y);
        unsafe {
            ffi::ghostty_tracked_grid_ref_free(self.tracked_row);
            self.tracked_row = ptr::null_mut();
            let _ = ffi::ghostty_terminal_grid_ref_track(terminal, target, &mut self.tracked_row);
        }
        (result == ffi::GhosttyResult_GHOSTTY_SUCCESS).then_some(point.y as usize)
    }

    pub fn screen_cell(&self, x: u16, y: u32) -> Result<(CellWide, Vec<u32>), Error> {
        let grid_ref = self.grid_ref(ghostty_screen_point(x, y))?;
        let wide = grid_ref_wide(&grid_ref)?;
        let graphemes = grid_ref_graphemes(&grid_ref)?;
        Ok((wide, graphemes))
    }

    pub(crate) fn screen_text_rows(&self) -> Result<Vec<ScreenTextRow>, Error> {
        self.screen_text_rows_range(0, usize::MAX)
    }

    pub(crate) fn screen_text_rows_range(
        &self,
        start_row: usize,
        end_row_exclusive: usize,
    ) -> Result<Vec<ScreenTextRow>, Error> {
        let total_rows = self.total_rows()?;
        let start_row = start_row.min(total_rows);
        let end_row_exclusive = end_row_exclusive.min(total_rows).max(start_row);
        let cols = self.cols()?;
        let mut rows = Vec::with_capacity(end_row_exclusive.saturating_sub(start_row));
        for y in start_row..end_row_exclusive {
            let Some(y) = u32::try_from(y).ok() else {
                break;
            };
            let mut grid_ref = self.grid_ref(ghostty_screen_point(0, y))?;
            let (soft_wrapped, wrap_continuation) = grid_ref_wrap_state(&grid_ref)?;
            let mut cells = Vec::with_capacity(usize::from(cols));
            for x in 0..cols {
                grid_ref.x = x;
                cells.push(ScreenTextCell {
                    wide: grid_ref_wide(&grid_ref)?,
                    graphemes: grid_ref_graphemes(&grid_ref)?,
                });
            }
            rows.push(ScreenTextRow {
                cells,
                soft_wrapped,
                wrap_continuation,
            });
        }
        Ok(rows)
    }

    pub(super) fn viewport_graphemes_and_style(
        &self,
        x: u16,
        y: u32,
    ) -> Result<(Vec<u32>, CellStyle), Error> {
        let grid_ref = self.grid_ref(ghostty_viewport_point(x, y))?;
        let graphemes = grid_ref_graphemes(&grid_ref)?;
        let mut style = ffi::GhosttyStyle {
            size: mem::size_of::<ffi::GhosttyStyle>(),
            ..Default::default()
        };
        unsafe {
            ffi::ghostty_grid_ref_style(&grid_ref, &mut style).into_result()?;
        }
        Ok((graphemes, style.into()))
    }

    pub fn viewport_hyperlink_uri(&self, x: u16, y: u32) -> Result<Option<String>, Error> {
        let grid_ref = self.grid_ref(ghostty_viewport_point(x, y))?;
        grid_ref_hyperlink_uri(&grid_ref)
    }

    fn grid_ref(&self, point: ffi::GhosttyPoint) -> Result<ffi::GhosttyGridRef, Error> {
        let mut grid_ref = ffi::GhosttyGridRef {
            size: mem::size_of::<ffi::GhosttyGridRef>(),
            ..Default::default()
        };
        unsafe {
            ffi::ghostty_terminal_grid_ref(self.raw, point, &mut grid_ref).into_result()?;
        }
        Ok(grid_ref)
    }

    pub fn read_ansi_viewport(
        &self,
        start: (u16, u32),
        end: (u16, u32),
        rectangle: bool,
    ) -> Result<String, Error> {
        self.read_formatted_selection(
            ghostty_viewport_point(start.0, start.1),
            ghostty_viewport_point(end.0, end.1),
            rectangle,
            FormatterFormat::Vt,
            false,
        )
    }

    pub fn read_text_screen(
        &self,
        start: (u16, u32),
        end: (u16, u32),
        rectangle: bool,
    ) -> Result<String, Error> {
        self.read_formatted_selection(
            ghostty_screen_point(start.0, start.1),
            ghostty_screen_point(end.0, end.1),
            rectangle,
            FormatterFormat::Plain,
            true,
        )
    }

    pub fn read_ansi_screen(
        &self,
        start: (u16, u32),
        end: (u16, u32),
        rectangle: bool,
        unwrap: bool,
    ) -> Result<String, Error> {
        self.read_formatted_selection(
            ghostty_screen_point(start.0, start.1),
            ghostty_screen_point(end.0, end.1),
            rectangle,
            FormatterFormat::Vt,
            unwrap,
        )
    }

    fn read_formatted_selection(
        &self,
        start: ffi::GhosttyPoint,
        end: ffi::GhosttyPoint,
        rectangle: bool,
        format: FormatterFormat,
        unwrap: bool,
    ) -> Result<String, Error> {
        let mut start_ref = ffi::GhosttyGridRef {
            size: mem::size_of::<ffi::GhosttyGridRef>(),
            ..Default::default()
        };
        let mut end_ref = ffi::GhosttyGridRef {
            size: mem::size_of::<ffi::GhosttyGridRef>(),
            ..Default::default()
        };
        unsafe {
            ffi::ghostty_terminal_grid_ref(self.raw, start, &mut start_ref).into_result()?;
            ffi::ghostty_terminal_grid_ref(self.raw, end, &mut end_ref).into_result()?;
        }

        let selection = ffi::GhosttySelection {
            size: mem::size_of::<ffi::GhosttySelection>(),
            start: start_ref,
            end: end_ref,
            rectangle,
        };
        let mut formatter: ffi::GhosttyFormatter = ptr::null_mut();
        let options = ffi::GhosttyFormatterTerminalOptions {
            size: mem::size_of::<ffi::GhosttyFormatterTerminalOptions>(),
            emit: format.as_raw(),
            unwrap,
            trim: true,
            extra: ffi::GhosttyFormatterTerminalExtra {
                size: mem::size_of::<ffi::GhosttyFormatterTerminalExtra>(),
                screen: ffi::GhosttyFormatterScreenExtra {
                    size: mem::size_of::<ffi::GhosttyFormatterScreenExtra>(),
                    ..Default::default()
                },
                ..Default::default()
            },
            selection: &selection,
        };
        unsafe {
            ffi::ghostty_formatter_terminal_new(ptr::null(), &mut formatter, self.raw, options)
                .into_result()?;
        }

        let mut out_ptr = ptr::null_mut();
        let mut out_len = 0usize;
        let result = unsafe {
            ffi::ghostty_formatter_format_alloc(formatter, ptr::null(), &mut out_ptr, &mut out_len)
        };
        unsafe {
            ffi::ghostty_formatter_free(formatter);
        }
        result.into_result()?;

        let text = if out_len == 0 {
            String::new()
        } else {
            let bytes = unsafe { slice::from_raw_parts(out_ptr.cast_const(), out_len) };
            String::from_utf8_lossy(bytes).into_owned()
        };

        if !out_ptr.is_null() {
            unsafe {
                ffi::ghostty_free(ptr::null(), out_ptr, out_len);
            }
        }

        Ok(text)
    }

    pub fn scroll_viewport_bottom(&mut self) {
        let viewport = ffi::GhosttyTerminalScrollViewport {
            tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
            value: ffi::GhosttyTerminalScrollViewportValue::default(),
        };
        // SAFETY: self.raw is valid and viewport value matches the tag.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.raw, viewport);
        }
    }

    pub fn scroll_viewport_delta(&mut self, delta: isize) {
        let viewport = ffi::GhosttyTerminalScrollViewport {
            tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_DELTA,
            value: ffi::GhosttyTerminalScrollViewportValue { delta },
        };
        // SAFETY: self.raw is valid and viewport value matches the tag.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.raw, viewport);
        }
    }

    pub fn scroll_viewport_row(&mut self, row: usize) {
        let viewport = ffi::GhosttyTerminalScrollViewport {
            tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_ROW,
            value: ffi::GhosttyTerminalScrollViewportValue { row },
        };
        // SAFETY: self.raw is valid and viewport value matches the tag.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.raw, viewport);
        }
    }

    pub fn cols(&self) -> Result<u16, Error> {
        self.get_u16(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS)
    }

    pub fn rows(&self) -> Result<u16, Error> {
        self.get_u16(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ROWS)
    }

    pub fn cursor_y(&self) -> Result<u16, Error> {
        self.get_u16(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_Y)
    }

    pub fn effective_foreground_color(&self) -> Result<Option<RgbColor>, Error> {
        self.get_optional_rgb_color(TERMINAL_DATA_COLOR_FOREGROUND)
    }

    pub fn effective_background_color(&self) -> Result<Option<RgbColor>, Error> {
        self.get_optional_rgb_color(TERMINAL_DATA_COLOR_BACKGROUND)
    }

    pub fn effective_cursor_color(&self) -> Result<Option<RgbColor>, Error> {
        self.get_optional_rgb_color(TERMINAL_DATA_COLOR_CURSOR)
    }

    pub(crate) fn width_px(&self) -> Result<u32, Error> {
        self.get_u32(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_WIDTH_PX)
    }

    pub(crate) fn height_px(&self) -> Result<u32, Error> {
        self.get_u32(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_HEIGHT_PX)
    }

    pub(super) fn get_u16(&self, data: ffi::GhosttyTerminalData) -> Result<u16, Error> {
        let mut out = 0u16;
        // SAFETY: out points to a u16 matching the requested terminal data type.
        unsafe {
            ffi::ghostty_terminal_get(self.raw, data, (&mut out as *mut u16).cast())
                .into_result()?;
        }
        Ok(out)
    }

    pub(super) fn get_u32(&self, data: ffi::GhosttyTerminalData) -> Result<u32, Error> {
        let mut out = 0u32;
        // SAFETY: out points to a u32 matching the requested terminal data type.
        unsafe {
            ffi::ghostty_terminal_get(self.raw, data, (&mut out as *mut u32).cast())
                .into_result()?;
        }
        Ok(out)
    }

    pub(super) fn get_usize(&self, data: ffi::GhosttyTerminalData) -> Result<usize, Error> {
        let mut out = 0usize;
        unsafe {
            ffi::ghostty_terminal_get(self.raw, data, (&mut out as *mut usize).cast())
                .into_result()?;
        }
        Ok(out)
    }

    pub(super) fn get_bool(&self, data: ffi::GhosttyTerminalData) -> Result<bool, Error> {
        let mut out = false;
        unsafe {
            ffi::ghostty_terminal_get(self.raw, data, (&mut out as *mut bool).cast())
                .into_result()?;
        }
        Ok(out)
    }

    pub(super) fn get_optional_rgb_color(
        &self,
        data: ffi::GhosttyTerminalData,
    ) -> Result<Option<RgbColor>, Error> {
        let mut out = ffi::GhosttyColorRgb::default();
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                data,
                (&mut out as *mut ffi::GhosttyColorRgb).cast(),
            )
        };
        match result {
            ffi::GhosttyResult_GHOSTTY_SUCCESS => Ok(Some(out.into())),
            ffi::GhosttyResult_GHOSTTY_NO_VALUE => Ok(None),
            other => Err(Error(other)),
        }
    }
}

fn ghostty_viewport_point(x: u16, y: u32) -> ffi::GhosttyPoint {
    ffi::GhosttyPoint {
        tag: ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_VIEWPORT,
        value: ffi::GhosttyPointValue {
            coordinate: ffi::GhosttyPointCoordinate { x, y },
        },
    }
}

fn ghostty_screen_point(x: u16, y: u32) -> ffi::GhosttyPoint {
    ffi::GhosttyPoint {
        tag: ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN,
        value: ffi::GhosttyPointValue {
            coordinate: ffi::GhosttyPointCoordinate { x, y },
        },
    }
}

fn grid_ref_graphemes(grid_ref: &ffi::GhosttyGridRef) -> Result<Vec<u32>, Error> {
    let mut required = 0usize;
    let result =
        unsafe { ffi::ghostty_grid_ref_graphemes(grid_ref, ptr::null_mut(), 0, &mut required) };
    if result != ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        result.into_result()?;
    }
    let mut buffer = vec![0u32; required];
    if required == 0 {
        return Ok(buffer);
    }
    unsafe {
        ffi::ghostty_grid_ref_graphemes(grid_ref, buffer.as_mut_ptr(), buffer.len(), &mut required)
            .into_result()?;
    }
    buffer.truncate(required);
    Ok(buffer)
}

fn grid_ref_wide(grid_ref: &ffi::GhosttyGridRef) -> Result<CellWide, Error> {
    let mut raw = ffi::GhosttyCell::default();
    unsafe {
        ffi::ghostty_grid_ref_cell(grid_ref, &mut raw).into_result()?;
    }

    let mut wide = ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
    unsafe {
        ffi::ghostty_cell_get(
            raw,
            ffi::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE,
            (&mut wide as *mut ffi::GhosttyCellWide).cast(),
        )
        .into_result()?;
    }
    Ok(CellWide::from_raw(wide))
}

fn grid_ref_wrap_state(grid_ref: &ffi::GhosttyGridRef) -> Result<(bool, bool), Error> {
    let mut row = 0;
    unsafe {
        ffi::ghostty_grid_ref_row(grid_ref, &mut row).into_result()?;
    }
    let mut soft_wrapped = false;
    let mut wrap_continuation = false;
    unsafe {
        ffi::ghostty_row_get(
            row,
            ffi::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP,
            (&mut soft_wrapped as *mut bool).cast(),
        )
        .into_result()?;
        ffi::ghostty_row_get(
            row,
            ffi::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP_CONTINUATION,
            (&mut wrap_continuation as *mut bool).cast(),
        )
        .into_result()?;
    }
    Ok((soft_wrapped, wrap_continuation))
}

fn grid_ref_hyperlink_uri(grid_ref: &ffi::GhosttyGridRef) -> Result<Option<String>, Error> {
    let mut required = 0usize;
    let result =
        unsafe { ffi::ghostty_grid_ref_hyperlink_uri(grid_ref, ptr::null_mut(), 0, &mut required) };
    if result != ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        result.into_result()?;
    }
    if required == 0 {
        return Ok(None);
    }
    let mut buffer = vec![0u8; required];
    unsafe {
        ffi::ghostty_grid_ref_hyperlink_uri(
            grid_ref,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut required,
        )
        .into_result()?;
    }
    buffer.truncate(required);
    Ok(Some(String::from_utf8_lossy(&buffer).into_owned()))
}
