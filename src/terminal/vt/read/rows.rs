use super::super::{
    ffi, CellColor, CellStyle, CellWide, Error, GhosttyResultExt, RenderState, RgbColor,
    RowSelection,
};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::{mem, ptr};

pub struct RowIterator {
    pub(in crate::terminal::vt) raw: ffi::GhosttyRenderStateRowIterator,
}

impl RowIterator {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        // SAFETY: valid out pointer and null allocator use default allocator.
        unsafe {
            ffi::ghostty_render_state_row_iterator_new(ptr::null(), &mut raw).into_result()?;
        }
        Ok(Self { raw })
    }
}

// SAFETY: these opaque handles are only used behind external synchronization in pane runtime.
unsafe impl Send for RowIterator {}

impl Drop for RowIterator {
    fn drop(&mut self) {
        // SAFETY: freeing a null or live handle is allowed by the C API.
        unsafe {
            ffi::ghostty_render_state_row_iterator_free(self.raw);
        }
    }
}

pub struct RowIter<'a> {
    pub(in crate::terminal::vt) iterator: &'a mut RowIterator,
    pub(in crate::terminal::vt) _state: PhantomData<&'a RenderState>,
}

impl<'a> RowIter<'a> {
    pub fn next(&mut self) -> bool {
        // SAFETY: iterator handle is valid while self is alive.
        unsafe { ffi::ghostty_render_state_row_iterator_next(self.iterator.raw) }
    }

    pub fn dirty(&self) -> Result<bool, Error> {
        let mut dirty = false;
        // SAFETY: dirty output matches requested row data type.
        unsafe {
            ffi::ghostty_render_state_row_get(
                self.iterator.raw,
                ffi::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                (&mut dirty as *mut bool).cast(),
            )
            .into_result()?;
        }
        Ok(dirty)
    }

    #[cfg(windows)]
    pub fn wrap_state(&self) -> Result<(bool, bool), Error> {
        let mut row = 0;
        // SAFETY: row output matches requested row data type.
        unsafe {
            ffi::ghostty_render_state_row_get(
                self.iterator.raw,
                ffi::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
                (&mut row as *mut ffi::GhosttyRow).cast(),
            )
            .into_result()?;
        }
        let mut soft_wrapped = false;
        // SAFETY: wrap output matches requested row data type.
        unsafe {
            ffi::ghostty_row_get(
                row,
                ffi::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP,
                (&mut soft_wrapped as *mut bool).cast(),
            )
            .into_result()?;
        }
        let mut wrap_continuation = false;
        // SAFETY: wrap continuation output matches requested row data type.
        unsafe {
            ffi::ghostty_row_get(
                row,
                ffi::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP_CONTINUATION,
                (&mut wrap_continuation as *mut bool).cast(),
            )
            .into_result()?;
        }
        Ok((soft_wrapped, wrap_continuation))
    }

    pub fn clear_dirty(&mut self) -> Result<(), Error> {
        self.set_dirty(false)
    }

    pub fn set_dirty(&mut self, dirty: bool) -> Result<(), Error> {
        // SAFETY: dirty pointer matches the expected row option type.
        unsafe {
            ffi::ghostty_render_state_row_set(
                self.iterator.raw,
                ffi::GhosttyRenderStateRowOption_GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY,
                (&dirty as *const bool).cast(),
            )
            .into_result()
        }
    }

    pub fn selection(&self) -> Result<Option<RowSelection>, Error> {
        let mut selection = ffi::GhosttyRenderStateRowSelection {
            size: mem::size_of::<ffi::GhosttyRenderStateRowSelection>(),
            ..Default::default()
        };
        let result = unsafe {
            ffi::ghostty_render_state_row_get(
                self.iterator.raw,
                ffi::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                (&mut selection as *mut ffi::GhosttyRenderStateRowSelection).cast(),
            )
        };
        match result {
            ffi::GhosttyResult_GHOSTTY_SUCCESS => Ok(Some(RowSelection {
                start_x: selection.start_x,
                end_x: selection.end_x,
            })),
            ffi::GhosttyResult_GHOSTTY_NO_VALUE => Ok(None),
            other => Err(Error(other)),
        }
    }

    pub fn populate_cells<'b>(
        &'b mut self,
        cells: &'b mut RowCells,
    ) -> Result<RowCellIter<'b>, Error> {
        // SAFETY: cells raw handle is valid and will not outlive the current row borrow.
        unsafe {
            ffi::ghostty_render_state_row_get(
                self.iterator.raw,
                ffi::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                (&mut cells.raw as *mut ffi::GhosttyRenderStateRowCells).cast(),
            )
            .into_result()?;
        }
        Ok(RowCellIter { cells })
    }
}

pub struct RowCells {
    pub(in crate::terminal::vt) raw: ffi::GhosttyRenderStateRowCells,
}

impl RowCells {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        // SAFETY: valid out pointer and null allocator use default allocator.
        unsafe {
            ffi::ghostty_render_state_row_cells_new(ptr::null(), &mut raw).into_result()?;
        }
        Ok(Self { raw })
    }
}

// SAFETY: these opaque handles are only used behind external synchronization in pane runtime.
unsafe impl Send for RowCells {}

impl Drop for RowCells {
    fn drop(&mut self) {
        // SAFETY: freeing a null or live handle is allowed by the C API.
        unsafe {
            ffi::ghostty_render_state_row_cells_free(self.raw);
        }
    }
}

pub struct RowCellIter<'a> {
    cells: &'a mut RowCells,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellBasicData {
    pub wide: CellWide,
    pub has_hyperlink: bool,
    pub has_styling: bool,
    pub style: CellStyle,
}

impl Default for CellBasicData {
    fn default() -> Self {
        Self {
            wide: CellWide::Narrow,
            has_hyperlink: false,
            has_styling: false,
            style: CellStyle::default(),
        }
    }
}

impl<'a> RowCellIter<'a> {
    pub fn next(&mut self) -> bool {
        // SAFETY: cells handle is valid while self is alive.
        unsafe { ffi::ghostty_render_state_row_cells_next(self.cells.raw) }
    }

    fn raw_cell(&self) -> Result<ffi::GhosttyCell, Error> {
        let mut raw = ffi::GhosttyCell::default();
        unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                (&mut raw as *mut ffi::GhosttyCell).cast(),
            )
            .into_result()?;
        }
        Ok(raw)
    }

    pub fn basic_data(&self) -> Result<CellBasicData, Error> {
        let mut raw = ffi::GhosttyCell::default();
        let mut style = ffi::GhosttyStyle {
            size: mem::size_of::<ffi::GhosttyStyle>(),
            ..Default::default()
        };
        let mut has_styling = false;
        let row_keys = [
            ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
            ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
            ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
        ];
        let mut row_values = [
            (&mut raw as *mut ffi::GhosttyCell).cast::<c_void>(),
            (&mut style as *mut ffi::GhosttyStyle).cast::<c_void>(),
            (&mut has_styling as *mut bool).cast::<c_void>(),
        ];
        let mut written = 0usize;
        unsafe {
            ffi::ghostty_render_state_row_cells_get_multi(
                self.cells.raw,
                row_keys.len(),
                row_keys.as_ptr(),
                row_values.as_mut_ptr(),
                &mut written,
            )
            .into_result()?;
        }

        let mut wide = ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
        let mut has_hyperlink = false;
        let cell_keys = [
            ffi::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE,
            ffi::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_HYPERLINK,
        ];
        let mut cell_values = [
            (&mut wide as *mut ffi::GhosttyCellWide).cast::<c_void>(),
            (&mut has_hyperlink as *mut bool).cast::<c_void>(),
        ];
        unsafe {
            ffi::ghostty_cell_get_multi(
                raw,
                cell_keys.len(),
                cell_keys.as_ptr(),
                cell_values.as_mut_ptr(),
                &mut written,
            )
            .into_result()?;
        }

        Ok(CellBasicData {
            wide: CellWide::from_raw(wide),
            has_hyperlink,
            has_styling,
            style: style.into(),
        })
    }

    pub fn wide(&self) -> Result<CellWide, Error> {
        let raw = self.raw_cell()?;
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

    pub fn has_hyperlink(&self) -> Result<bool, Error> {
        let raw = self.raw_cell()?;
        let mut has_hyperlink = false;
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_HYPERLINK,
                (&mut has_hyperlink as *mut bool).cast(),
            )
            .into_result()?;
        }
        Ok(has_hyperlink)
    }

    pub fn content_bg_color(&self) -> Result<Option<CellColor>, Error> {
        let raw = self.raw_cell()?;
        let mut tag = ffi::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT;
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_CONTENT_TAG,
                (&mut tag as *mut ffi::GhosttyCellContentTag).cast(),
            )
            .into_result()?;
        }

        match tag {
            ffi::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => {
                let mut index = 0u8;
                unsafe {
                    ffi::ghostty_cell_get(
                        raw,
                        ffi::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_PALETTE,
                        (&mut index as *mut u8).cast(),
                    )
                    .into_result()?;
                }
                Ok(Some(CellColor::Palette(index)))
            }
            ffi::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => {
                let mut color = ffi::GhosttyColorRgb::default();
                unsafe {
                    ffi::ghostty_cell_get(
                        raw,
                        ffi::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_RGB,
                        (&mut color as *mut ffi::GhosttyColorRgb).cast(),
                    )
                    .into_result()?;
                }
                Ok(Some(CellColor::Rgb(color.into())))
            }
            _ => Ok(None),
        }
    }

    pub fn fg_color(&self) -> Result<Option<RgbColor>, Error> {
        let mut color = ffi::GhosttyColorRgb::default();
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                (&mut color as *mut ffi::GhosttyColorRgb).cast(),
            )
        };
        match result {
            ffi::GhosttyResult_GHOSTTY_SUCCESS => Ok(Some(color.into())),
            ffi::GhosttyResult_GHOSTTY_INVALID_VALUE => Ok(None),
            other => Err(Error(other)),
        }
    }

    pub fn bg_color(&self) -> Result<Option<RgbColor>, Error> {
        let mut color = ffi::GhosttyColorRgb::default();
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                (&mut color as *mut ffi::GhosttyColorRgb).cast(),
            )
        };
        match result {
            ffi::GhosttyResult_GHOSTTY_SUCCESS => Ok(Some(color.into())),
            ffi::GhosttyResult_GHOSTTY_INVALID_VALUE => Ok(None),
            other => Err(Error(other)),
        }
    }

    fn raw_cell_text_into(&self, text: &mut String) -> Result<(), Error> {
        let raw = self.raw_cell()?;
        let mut has_text = false;
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_TEXT,
                (&mut has_text as *mut bool).cast(),
            )
            .into_result()?;
        }
        if !has_text {
            return Ok(());
        }

        let mut codepoint = 0u32;
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_CODEPOINT,
                (&mut codepoint as *mut u32).cast(),
            )
            .into_result()?;
        }
        if let Some(ch) = char::from_u32(codepoint) {
            text.push(ch);
        }
        Ok(())
    }

    pub fn grapheme_text(&self) -> Result<String, Error> {
        let mut bytes = Vec::new();
        let mut text = String::new();
        self.grapheme_text_into(&mut bytes, &mut text)?;
        Ok(text)
    }

    pub fn grapheme_text_into(&self, bytes: &mut Vec<u8>, text: &mut String) -> Result<(), Error> {
        text.clear();
        bytes.clear();

        let mut buffer = ffi::GhosttyBuffer {
            ptr: ptr::null_mut(),
            cap: 0,
            len: 0,
        };
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                (&mut buffer as *mut ffi::GhosttyBuffer).cast(),
            )
        };
        match result {
            ffi::GhosttyResult_GHOSTTY_SUCCESS if buffer.len == 0 => {
                return self.raw_cell_text_into(text);
            }
            ffi::GhosttyResult_GHOSTTY_SUCCESS => {
                return Err(Error(ffi::GhosttyResult_GHOSTTY_INVALID_VALUE));
            }
            ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE => {}
            other => return Err(Error(other)),
        }

        if buffer.len == 0 {
            return self.raw_cell_text_into(text);
        }
        bytes.resize(buffer.len, 0);
        let mut buffer = ffi::GhosttyBuffer {
            ptr: bytes.as_mut_ptr(),
            cap: bytes.len(),
            len: 0,
        };
        unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                (&mut buffer as *mut ffi::GhosttyBuffer).cast(),
            )
            .into_result()?;
        }
        if buffer.len > bytes.len() {
            return Err(Error(ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE));
        }
        bytes.truncate(buffer.len);
        match std::str::from_utf8(bytes) {
            Ok(value) => text.push_str(value),
            Err(_) => text.push_str(&String::from_utf8_lossy(bytes)),
        }
        Ok(())
    }
}
