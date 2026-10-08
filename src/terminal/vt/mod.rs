#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all,
    rustdoc::all
)]
pub mod ffi;

mod callbacks;
mod consts;
mod read;
mod types;

use callbacks::{
    bell_trampoline, clipboard_write_trampoline, color_scheme_trampoline, install_png_decoder_once,
    pwd_changed_trampoline, size_trampoline, write_pty_trampoline, TerminalCallbackState,
};
pub(crate) use consts::KITTY_UNICODE_PLACEHOLDER;
#[cfg(test)]
pub use consts::MODE_GRAPHEME_CLUSTER;
use consts::{
    APC_MAX_BYTES, APC_MAX_BYTES_KITTY, KITTY_IMAGE_STORAGE_LIMIT_BYTES,
    KITTY_PLACEMENT_DATA_COLUMNS, KITTY_PLACEMENT_DATA_IS_VIRTUAL, KITTY_PLACEMENT_DATA_ROWS,
};
pub use consts::{
    MODE_APPLICATION_CURSOR_KEYS, MODE_BRACKETED_PASTE, MODE_COLOR_SCHEME_REPORT, MODE_FOCUS_EVENT,
    MODE_MOUSE_ALTERNATE_SCROLL, MODE_MOUSE_SGR_PIXELS, MODE_SYNCHRONIZED_OUTPUT, MOD_ALT,
    MOD_CTRL, MOD_SHIFT, MOD_SUPER, MOUSE_ACTION_MOTION, MOUSE_ACTION_PRESS, MOUSE_ACTION_RELEASE,
    MOUSE_BUTTON_LEFT, MOUSE_BUTTON_MIDDLE, MOUSE_BUTTON_RIGHT, MOUSE_BUTTON_WHEEL_DOWN,
    MOUSE_BUTTON_WHEEL_LEFT, MOUSE_BUTTON_WHEEL_RIGHT, MOUSE_BUTTON_WHEEL_UP, MOUSE_FORMAT_SGR,
    MOUSE_FORMAT_SGR_PIXELS,
};
pub use read::{CellBasicData, RowCellIter, RowCells, RowIter, RowIterator};
use types::GhosttyResultExt;
pub use types::{
    default_palette, ActiveScreen, CellColor, CellStyle, CellWide, ColorScheme, CursorViewport,
    CursorVisualStyle, Dirty, Error, FocusEvent, RenderColors, RgbColor, RowSelection,
    TerminalScrollbar,
};
pub(crate) use types::{ScreenTextCell, ScreenTextRow, TerminalCompressionResult};

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::mem;
use std::os::raw::c_char;
use std::ptr;
use std::slice;
use std::sync::{Mutex, OnceLock};

static KITTY_PLACEHOLDER_DIACRITICS: OnceLock<HashMap<u32, u32>> = OnceLock::new();

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum KittyImageFormat {
    Rgb,
    Rgba,
    Png,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyImagePlacement {
    pub image_id: u32,
    pub placement_id: u32,
    pub z: i32,
    pub x_offset: u32,
    pub y_offset: u32,
    pub image_width: u32,
    pub image_height: u32,
    pub format: KittyImageFormat,
    pub data_len: usize,
    pub data_fingerprint: u64,
    pub data: Vec<u8>,
    pub render: KittyPlacementRenderInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KittyImageDescriptor {
    pub image_id: u32,
    pub placement_id: u32,
    pub image_width: u32,
    pub image_height: u32,
    pub format: KittyImageFormat,
    pub data_len: usize,
    pub data_fingerprint: u64,
}

#[derive(Debug, Clone, Copy)]
struct KittyImageFingerprintEntry {
    generation: u64,
    fingerprint: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KittyPlacementRenderInfo {
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub viewport_col: i32,
    pub viewport_row: i32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyVirtualPlacementSpec {
    image_id: u32,
    placement_id: u32,
    columns: u32,
    rows: u32,
    z: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyVirtualCell {
    x: u16,
    y: u16,
    image_id_low: u32,
    image_id_high: Option<u32>,
    placement_id: Option<u32>,
    row: Option<u32>,
    col: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyVirtualRun {
    x: u16,
    y: u16,
    image_id_low: u32,
    image_id_high: Option<u32>,
    placement_id: Option<u32>,
    row: u32,
    col: u32,
    width: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyVirtualPlacementGeometry {
    x_offset: u32,
    y_offset: u32,
    render: KittyPlacementRenderInfo,
}

pub fn unicode_codepoint_width(codepoint: u32) -> u8 {
    unsafe { ffi::ghostty_unicode_codepoint_width(codepoint) }
}

pub fn encode_focus(event: FocusEvent) -> Result<Vec<u8>, Error> {
    let mut required = 0usize;
    // SAFETY: null buffer + out len is the documented way to query required size.
    let result =
        unsafe { ffi::ghostty_focus_encode(event.as_raw(), ptr::null_mut(), 0, &mut required) };
    if result != ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        result.into_result()?;
    }

    let mut buffer = vec![0u8; required];
    // SAFETY: buffer is allocated for required size; function writes at most that many bytes.
    unsafe {
        ffi::ghostty_focus_encode(
            event.as_raw(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut required,
        )
        .into_result()?;
    }
    buffer.truncate(required);
    Ok(buffer)
}

pub struct Terminal {
    raw: ffi::GhosttyTerminal,
    #[cfg(windows)]
    max_scrollback: usize,
    #[cfg(windows)]
    tracked_row: ffi::GhosttyTrackedGridRef,
    callback_state: Box<TerminalCallbackState>,
    kitty_fingerprints: Mutex<HashMap<u32, KittyImageFingerprintEntry>>,
    kitty_empty_generation: Cell<Option<u64>>,
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

    pub fn enable_kitty_graphics(&mut self) -> Result<(), Error> {
        install_png_decoder_once();
        let storage_limit = KITTY_IMAGE_STORAGE_LIMIT_BYTES;
        let enable_medium = true;
        unsafe {
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
                (&storage_limit as *const u64).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_FILE,
                (&enable_medium as *const bool).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE,
                (&enable_medium as *const bool).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM,
                (&enable_medium as *const bool).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES,
                (&APC_MAX_BYTES as *const usize).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES_KITTY,
                (&APC_MAX_BYTES_KITTY as *const usize).cast(),
            )
            .into_result()?;
        }
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

    fn kitty_graphics(&self) -> Result<ffi::GhosttyKittyGraphics, Error> {
        let mut graphics: ffi::GhosttyKittyGraphics = ptr::null_mut();
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS,
                (&mut graphics as *mut ffi::GhosttyKittyGraphics).cast(),
            )
            .into_result()?;
        }
        Ok(graphics)
    }

    pub fn kitty_graphics_generation(&self) -> Result<u64, Error> {
        let graphics = self.kitty_graphics()?;
        if graphics.is_null() {
            return Ok(0);
        }
        kitty_graphics_u64(
            graphics,
            ffi::GhosttyKittyGraphicsData_GHOSTTY_KITTY_GRAPHICS_DATA_GENERATION,
        )
    }

    pub(crate) fn kitty_graphics_may_have_placements(&self) -> Result<bool, Error> {
        let generation = self.kitty_graphics_generation()?;
        Ok(generation != 0 && self.kitty_empty_generation.get() != Some(generation))
    }

    pub fn kitty_image_placements_with_data_filter<F>(
        &self,
        mut needs_data: F,
    ) -> Result<Vec<KittyImagePlacement>, Error>
    where
        F: FnMut(KittyImageDescriptor) -> bool,
    {
        let graphics = self.kitty_graphics()?;
        if graphics.is_null() {
            return Ok(Vec::new());
        }
        let generation = kitty_graphics_u64(
            graphics,
            ffi::GhosttyKittyGraphicsData_GHOSTTY_KITTY_GRAPHICS_DATA_GENERATION,
        )?;
        if generation == 0 || self.kitty_empty_generation.get() == Some(generation) {
            return Ok(Vec::new());
        }

        let guard = KittyPlacementIteratorGuard::new(graphics)?;
        let iterator = guard.raw;

        let mut placements = Vec::new();
        let mut storage_has_placements = false;
        while unsafe { ffi::ghostty_kitty_graphics_placement_next(iterator) } {
            storage_has_placements = true;
            if let Some(placement) =
                self.kitty_image_placement(graphics, iterator, &mut needs_data)?
            {
                placements.push(placement);
            }
        }
        if !storage_has_placements {
            self.kitty_empty_generation.set(Some(generation));
            self.prune_kitty_fingerprints(&[]);
            return Ok(Vec::new());
        }

        placements.extend(self.kitty_virtual_image_placements(graphics, &mut needs_data)?);
        placements.sort_by_key(|placement| placement.z);
        self.prune_kitty_fingerprints(&placements);
        Ok(placements)
    }

    /// Fingerprint for `image`, cached per image id and recomputed only when
    /// the image's generation changes.
    fn kitty_image_fingerprint_cached(
        &self,
        image: ffi::GhosttyKittyGraphicsImage,
        image_id: u32,
        data: (*const u8, usize),
        image_width: u32,
        image_height: u32,
        format: KittyImageFormat,
    ) -> u64 {
        let (data_ptr, data_len) = data;
        let Ok(generation) = kitty_image_u64(
            image,
            ffi::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_GENERATION,
        ) else {
            return kitty_image_fingerprint(data_ptr, data_len, image_width, image_height, format);
        };

        if let Ok(cache) = self.kitty_fingerprints.lock() {
            if let Some(entry) = cache.get(&image_id) {
                if entry.generation == generation {
                    return entry.fingerprint;
                }
            }
        }

        let fingerprint =
            kitty_image_fingerprint(data_ptr, data_len, image_width, image_height, format);
        if let Ok(mut cache) = self.kitty_fingerprints.lock() {
            cache.insert(
                image_id,
                KittyImageFingerprintEntry {
                    generation,
                    fingerprint,
                },
            );
        }
        fingerprint
    }

    fn prune_kitty_fingerprints(&self, placements: &[KittyImagePlacement]) {
        if let Ok(mut cache) = self.kitty_fingerprints.lock() {
            if cache.is_empty() {
                return;
            }
            let live: HashSet<u32> = placements
                .iter()
                .map(|placement| placement.image_id)
                .collect();
            cache.retain(|image_id, _| live.contains(image_id));
        }
    }

    fn kitty_image_placement<F>(
        &self,
        graphics: ffi::GhosttyKittyGraphics,
        iterator: ffi::GhosttyKittyGraphicsPlacementIterator,
        needs_data: &mut F,
    ) -> Result<Option<KittyImagePlacement>, Error>
    where
        F: FnMut(KittyImageDescriptor) -> bool,
    {
        let image_id = kitty_placement_u32(
            iterator,
            ffi::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID,
        )?;
        if kitty_placement_bool(iterator, KITTY_PLACEMENT_DATA_IS_VIRTUAL)? {
            return Ok(None);
        }
        let image = unsafe { ffi::ghostty_kitty_graphics_image(graphics, image_id) };
        if image.is_null() {
            return Ok(None);
        }

        let mut raw_info = ffi::GhosttyKittyGraphicsPlacementRenderInfo {
            size: mem::size_of::<ffi::GhosttyKittyGraphicsPlacementRenderInfo>(),
            ..Default::default()
        };
        unsafe {
            ffi::ghostty_kitty_graphics_placement_render_info(
                iterator,
                image,
                self.raw,
                &mut raw_info,
            )
            .into_result()?;
        }
        if !raw_info.viewport_visible {
            return Ok(None);
        }

        let image_width = kitty_image_u32(
            image,
            ffi::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_WIDTH,
        )?;
        let image_height = kitty_image_u32(
            image,
            ffi::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_HEIGHT,
        )?;
        let format = kitty_image_format(image)?;
        let compression = kitty_image_compression(image)?;
        if compression != ffi::GhosttyKittyImageCompression_GHOSTTY_KITTY_IMAGE_COMPRESSION_NONE {
            return Ok(None);
        }
        let placement_id = kitty_placement_u32(
            iterator,
            ffi::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_PLACEMENT_ID,
        )?;
        let (data_ptr, data_len) = kitty_image_data_ptr_len(image)?;
        let data_fingerprint = self.kitty_image_fingerprint_cached(
            image,
            image_id,
            (data_ptr, data_len),
            image_width,
            image_height,
            format,
        );
        let descriptor = KittyImageDescriptor {
            image_id,
            placement_id,
            image_width,
            image_height,
            format,
            data_len,
            data_fingerprint,
        };
        let data = if needs_data(descriptor) {
            kitty_image_data_from_ptr(data_ptr, data_len)
        } else {
            Vec::new()
        };
        let x_offset = kitty_placement_u32(
            iterator,
            ffi::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_X_OFFSET,
        )?;
        let y_offset = kitty_placement_u32(
            iterator,
            ffi::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Y_OFFSET,
        )?;
        let z = kitty_placement_i32(
            iterator,
            ffi::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z,
        )?;

        Ok(Some(KittyImagePlacement {
            image_id,
            placement_id,
            z,
            x_offset,
            y_offset,
            image_width,
            image_height,
            format,
            data_len,
            data_fingerprint,
            data,
            render: KittyPlacementRenderInfo {
                pixel_width: raw_info.pixel_width,
                pixel_height: raw_info.pixel_height,
                grid_cols: raw_info.grid_cols,
                grid_rows: raw_info.grid_rows,
                viewport_col: raw_info.viewport_col,
                viewport_row: raw_info.viewport_row,
                source_x: raw_info.source_x,
                source_y: raw_info.source_y,
                source_width: raw_info.source_width,
                source_height: raw_info.source_height,
            },
        }))
    }

    fn kitty_virtual_image_placements<F>(
        &self,
        graphics: ffi::GhosttyKittyGraphics,
        needs_data: &mut F,
    ) -> Result<Vec<KittyImagePlacement>, Error>
    where
        F: FnMut(KittyImageDescriptor) -> bool,
    {
        let specs = kitty_virtual_placement_specs(graphics)?;
        if specs.is_empty() {
            return Ok(Vec::new());
        }

        let viewport_cols = self.cols()?.max(1);
        let viewport_rows = self.rows()?.max(1);
        let cell_width = (self.width_px()? / u32::from(viewport_cols)).max(1);
        let cell_height = (self.height_px()? / u32::from(viewport_rows)).max(1);
        let mut runs = Vec::new();
        for y in 0..viewport_rows {
            let mut current: Option<KittyVirtualRun> = None;
            for x in 0..viewport_cols {
                let (graphemes, style) = self.viewport_graphemes_and_style(x, u32::from(y))?;
                let cell = kitty_virtual_cell(x, y, &graphemes, style);
                match cell {
                    Some(cell) => {
                        if let Some(run) = current.as_mut() {
                            if run.append(cell) {
                                continue;
                            }
                            runs.push(*run);
                        }
                        current = Some(KittyVirtualRun::from_cell(cell));
                    }
                    None => {
                        if let Some(run) = current.take() {
                            runs.push(run);
                        }
                    }
                }
            }
            if let Some(run) = current {
                runs.push(run);
            }
        }

        let mut placements = Vec::new();
        for run in runs {
            let image_id = run.image_id();
            let Some(spec) = find_virtual_placement_spec(&specs, image_id, run.placement_id())
            else {
                continue;
            };
            let image = unsafe { ffi::ghostty_kitty_graphics_image(graphics, image_id) };
            if image.is_null() {
                continue;
            }
            let image_width = kitty_image_u32(
                image,
                ffi::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_WIDTH,
            )?;
            let image_height = kitty_image_u32(
                image,
                ffi::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_HEIGHT,
            )?;
            let format = kitty_image_format(image)?;
            let compression = kitty_image_compression(image)?;
            if compression != ffi::GhosttyKittyImageCompression_GHOSTTY_KITTY_IMAGE_COMPRESSION_NONE
            {
                continue;
            }
            let Some(geometry) = kitty_virtual_placement_geometry(
                run,
                *spec,
                image_width,
                image_height,
                cell_width,
                cell_height,
            ) else {
                continue;
            };
            let placement_id = run.synthetic_placement_id();
            let (data_ptr, data_len) = kitty_image_data_ptr_len(image)?;
            let data_fingerprint = self.kitty_image_fingerprint_cached(
                image,
                image_id,
                (data_ptr, data_len),
                image_width,
                image_height,
                format,
            );
            let descriptor = KittyImageDescriptor {
                image_id,
                placement_id,
                image_width,
                image_height,
                format,
                data_len,
                data_fingerprint,
            };
            let data = if needs_data(descriptor) {
                kitty_image_data_from_ptr(data_ptr, data_len)
            } else {
                Vec::new()
            };
            placements.push(KittyImagePlacement {
                image_id,
                placement_id,
                z: spec.z,
                x_offset: geometry.x_offset,
                y_offset: geometry.y_offset,
                image_width,
                image_height,
                format,
                data_len,
                data_fingerprint,
                data,
                render: geometry.render,
            });
        }

        Ok(placements)
    }

    fn raw(&self) -> ffi::GhosttyTerminal {
        self.raw
    }
}

struct KittyPlacementIteratorGuard {
    raw: ffi::GhosttyKittyGraphicsPlacementIterator,
}

impl KittyPlacementIteratorGuard {
    fn new(graphics: ffi::GhosttyKittyGraphics) -> Result<Self, Error> {
        let mut iterator: ffi::GhosttyKittyGraphicsPlacementIterator = ptr::null_mut();
        unsafe {
            ffi::ghostty_kitty_graphics_placement_iterator_new(ptr::null(), &mut iterator)
                .into_result()?;
            ffi::ghostty_kitty_graphics_get(
                graphics,
                ffi::GhosttyKittyGraphicsData_GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR,
                (&mut iterator as *mut ffi::GhosttyKittyGraphicsPlacementIterator).cast(),
            )
            .into_result()?;
        }
        Ok(Self { raw: iterator })
    }
}

impl Drop for KittyPlacementIteratorGuard {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_kitty_graphics_placement_iterator_free(self.raw) }
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

fn kitty_placement_u32(
    iterator: ffi::GhosttyKittyGraphicsPlacementIterator,
    data: ffi::GhosttyKittyGraphicsPlacementData,
) -> Result<u32, Error> {
    let mut out = 0u32;
    unsafe {
        ffi::ghostty_kitty_graphics_placement_get(iterator, data, (&mut out as *mut u32).cast())
            .into_result()?;
    }
    Ok(out)
}

fn kitty_placement_i32(
    iterator: ffi::GhosttyKittyGraphicsPlacementIterator,
    data: ffi::GhosttyKittyGraphicsPlacementData,
) -> Result<i32, Error> {
    let mut out = 0i32;
    unsafe {
        ffi::ghostty_kitty_graphics_placement_get(iterator, data, (&mut out as *mut i32).cast())
            .into_result()?;
    }
    Ok(out)
}

fn kitty_placement_bool(
    iterator: ffi::GhosttyKittyGraphicsPlacementIterator,
    data: ffi::GhosttyKittyGraphicsPlacementData,
) -> Result<bool, Error> {
    let mut out = false;
    unsafe {
        ffi::ghostty_kitty_graphics_placement_get(iterator, data, (&mut out as *mut bool).cast())
            .into_result()?;
    }
    Ok(out)
}

fn kitty_virtual_placement_specs(
    graphics: ffi::GhosttyKittyGraphics,
) -> Result<Vec<KittyVirtualPlacementSpec>, Error> {
    let guard = KittyPlacementIteratorGuard::new(graphics)?;
    let iterator = guard.raw;

    let mut specs = Vec::new();
    while unsafe { ffi::ghostty_kitty_graphics_placement_next(iterator) } {
        if !kitty_placement_bool(iterator, KITTY_PLACEMENT_DATA_IS_VIRTUAL)? {
            continue;
        }
        specs.push(KittyVirtualPlacementSpec {
            image_id: kitty_placement_u32(
                iterator,
                ffi::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID,
            )?,
            placement_id: kitty_placement_u32(
                iterator,
                ffi::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_PLACEMENT_ID,
            )?,
            columns: kitty_placement_u32(iterator, KITTY_PLACEMENT_DATA_COLUMNS)?,
            rows: kitty_placement_u32(iterator, KITTY_PLACEMENT_DATA_ROWS)?,
            z: kitty_placement_i32(
                iterator,
                ffi::GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z,
            )?,
        });
    }
    Ok(specs)
}

fn find_virtual_placement_spec(
    specs: &[KittyVirtualPlacementSpec],
    image_id: u32,
    placement_id: u32,
) -> Option<&KittyVirtualPlacementSpec> {
    if placement_id > 0 {
        specs
            .iter()
            .find(|spec| spec.image_id == image_id && spec.placement_id == placement_id)
    } else {
        specs.iter().find(|spec| spec.image_id == image_id)
    }
}

fn kitty_virtual_cell(
    x: u16,
    y: u16,
    graphemes: &[u32],
    style: CellStyle,
) -> Option<KittyVirtualCell> {
    if graphemes.first().copied() != Some(KITTY_UNICODE_PLACEHOLDER) {
        return None;
    }
    let image_id_low = style
        .fg_color
        .map(kitty_placeholder_color_to_id)
        .unwrap_or(0);
    let placement_id = style
        .underline_color
        .map(kitty_placeholder_color_to_id)
        .filter(|id| *id != 0);
    let row = graphemes
        .get(1)
        .and_then(|codepoint| kitty_placeholder_diacritic_index(*codepoint));
    let col = graphemes
        .get(2)
        .and_then(|codepoint| kitty_placeholder_diacritic_index(*codepoint));
    let image_id_high = graphemes
        .get(3)
        .and_then(|codepoint| kitty_placeholder_diacritic_index(*codepoint))
        .filter(|high| *high <= u32::from(u8::MAX));

    Some(KittyVirtualCell {
        x,
        y,
        image_id_low,
        image_id_high,
        placement_id,
        row,
        col,
    })
}

fn kitty_placeholder_color_to_id(color: CellColor) -> u32 {
    match color {
        CellColor::Palette(value) => value.into(),
        CellColor::Rgb(color) => {
            (u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)
        }
    }
}

fn kitty_placeholder_diacritic_index(codepoint: u32) -> Option<u32> {
    let map = KITTY_PLACEHOLDER_DIACRITICS.get_or_init(|| {
        // Reuse Ghostty's vendored table so Herdr decodes the same placeholder
        // row/column diacritics that libghostty accepts.
        let source =
            include_str!("../../../vendor/libghostty-vt/src/terminal/kitty/graphics_unicode.zig");
        let mut map = HashMap::new();
        let mut in_table = false;
        for line in source.lines() {
            let line = line.trim();
            if line.starts_with("const diacritics:") {
                in_table = true;
                continue;
            }
            if !in_table {
                continue;
            }
            if line == "};" {
                break;
            }
            let Some(hex) = line
                .strip_prefix("0x")
                .and_then(|value| value.strip_suffix(','))
            else {
                continue;
            };
            if let Ok(value) = u32::from_str_radix(hex, 16) {
                map.insert(value, map.len() as u32);
            }
        }
        map
    });
    map.get(&codepoint).copied()
}

fn kitty_virtual_placement_geometry(
    run: KittyVirtualRun,
    spec: KittyVirtualPlacementSpec,
    image_width: u32,
    image_height: u32,
    cell_width: u32,
    cell_height: u32,
) -> Option<KittyVirtualPlacementGeometry> {
    let grid_cols = if spec.columns == 0 {
        image_width.saturating_add(cell_width - 1) / cell_width
    } else {
        spec.columns
    }
    .max(1);
    let grid_rows = if spec.rows == 0 {
        image_height.saturating_add(cell_height - 1) / cell_height
    } else {
        spec.rows
    }
    .max(1);

    if run.col >= grid_cols || run.row >= grid_rows {
        return None;
    }
    let visible_cols = run.width.min(grid_cols.saturating_sub(run.col)).max(1);
    let visible_rows = 1;
    let source_x = scale_u32(run.col, image_width, grid_cols);
    let source_y = scale_u32(run.row, image_height, grid_rows);
    let source_width = scale_u32(visible_cols, image_width, grid_cols)
        .max(1)
        .min(image_width.saturating_sub(source_x));
    let source_height = scale_u32(visible_rows, image_height, grid_rows)
        .max(1)
        .min(image_height.saturating_sub(source_y));
    if source_width == 0 || source_height == 0 {
        return None;
    }

    Some(KittyVirtualPlacementGeometry {
        x_offset: 0,
        y_offset: 0,
        render: KittyPlacementRenderInfo {
            pixel_width: visible_cols.saturating_mul(cell_width).max(1),
            pixel_height: visible_rows.saturating_mul(cell_height).max(1),
            grid_cols: visible_cols,
            grid_rows: visible_rows,
            viewport_col: i32::from(run.x),
            viewport_row: i32::from(run.y),
            source_x,
            source_y,
            source_width,
            source_height,
        },
    })
}

fn scale_u32(value: u32, source: u32, dest: u32) -> u32 {
    ((u64::from(value)).saturating_mul(u64::from(source)) / u64::from(dest.max(1)))
        .min(u64::from(u32::MAX)) as u32
}

impl KittyVirtualRun {
    fn from_cell(cell: KittyVirtualCell) -> Self {
        Self {
            x: cell.x,
            y: cell.y,
            image_id_low: cell.image_id_low,
            image_id_high: cell.image_id_high,
            placement_id: cell.placement_id,
            row: cell.row.unwrap_or(0),
            col: cell.col.unwrap_or(0),
            width: 1,
        }
    }

    fn append(&mut self, cell: KittyVirtualCell) -> bool {
        if self.image_id_low != cell.image_id_low
            || self.placement_id != cell.placement_id
            || cell.row.is_some_and(|row| row != self.row)
            || cell.col.is_some_and(|col| col != self.col + self.width)
            || cell
                .image_id_high
                .is_some_and(|high| Some(high) != self.image_id_high)
        {
            return false;
        }
        self.width += 1;
        true
    }

    fn image_id(self) -> u32 {
        self.image_id_low | (self.image_id_high.unwrap_or(0) << 24)
    }

    fn placement_id(self) -> u32 {
        self.placement_id.unwrap_or(0)
    }

    fn synthetic_placement_id(self) -> u32 {
        let mut hasher = DefaultHasher::new();
        self.image_id().hash(&mut hasher);
        self.placement_id().hash(&mut hasher);
        self.row.hash(&mut hasher);
        self.col.hash(&mut hasher);
        self.width.hash(&mut hasher);
        self.x.hash(&mut hasher);
        self.y.hash(&mut hasher);
        1 + ((hasher.finish() as u32) % 900_000)
    }
}

fn kitty_graphics_u64(
    graphics: ffi::GhosttyKittyGraphics,
    data: ffi::GhosttyKittyGraphicsData,
) -> Result<u64, Error> {
    let mut out = 0u64;
    unsafe {
        ffi::ghostty_kitty_graphics_get(graphics, data, (&mut out as *mut u64).cast())
            .into_result()?;
    }
    Ok(out)
}

fn kitty_image_u32(
    image: ffi::GhosttyKittyGraphicsImage,
    data: ffi::GhosttyKittyGraphicsImageData,
) -> Result<u32, Error> {
    let mut out = 0u32;
    unsafe {
        ffi::ghostty_kitty_graphics_image_get(image, data, (&mut out as *mut u32).cast())
            .into_result()?;
    }
    Ok(out)
}

fn kitty_image_u64(
    image: ffi::GhosttyKittyGraphicsImage,
    data: ffi::GhosttyKittyGraphicsImageData,
) -> Result<u64, Error> {
    let mut out = 0u64;
    unsafe {
        ffi::ghostty_kitty_graphics_image_get(image, data, (&mut out as *mut u64).cast())
            .into_result()?;
    }
    Ok(out)
}

fn kitty_image_format(image: ffi::GhosttyKittyGraphicsImage) -> Result<KittyImageFormat, Error> {
    let mut out = ffi::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_RGBA;
    unsafe {
        ffi::ghostty_kitty_graphics_image_get(
            image,
            ffi::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_FORMAT,
            (&mut out as *mut ffi::GhosttyKittyImageFormat).cast(),
        )
        .into_result()?;
    }
    match out {
        ffi::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_RGB => Ok(KittyImageFormat::Rgb),
        ffi::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_RGBA => Ok(KittyImageFormat::Rgba),
        ffi::GhosttyKittyImageFormat_GHOSTTY_KITTY_IMAGE_FORMAT_PNG => Ok(KittyImageFormat::Png),
        _ => Err(Error(ffi::GhosttyResult_GHOSTTY_INVALID_VALUE)),
    }
}

fn kitty_image_compression(
    image: ffi::GhosttyKittyGraphicsImage,
) -> Result<ffi::GhosttyKittyImageCompression, Error> {
    let mut out = ffi::GhosttyKittyImageCompression_GHOSTTY_KITTY_IMAGE_COMPRESSION_NONE;
    unsafe {
        ffi::ghostty_kitty_graphics_image_get(
            image,
            ffi::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_COMPRESSION,
            (&mut out as *mut ffi::GhosttyKittyImageCompression).cast(),
        )
        .into_result()?;
    }
    Ok(out)
}

fn kitty_image_data_ptr_len(
    image: ffi::GhosttyKittyGraphicsImage,
) -> Result<(*const u8, usize), Error> {
    let mut ptr_out: *const u8 = ptr::null();
    let mut len = 0usize;
    unsafe {
        ffi::ghostty_kitty_graphics_image_get(
            image,
            ffi::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_DATA_PTR,
            (&mut ptr_out as *mut *const u8).cast(),
        )
        .into_result()?;
        ffi::ghostty_kitty_graphics_image_get(
            image,
            ffi::GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_IMAGE_DATA_DATA_LEN,
            (&mut len as *mut usize).cast(),
        )
        .into_result()?;
    }
    Ok((ptr_out, len))
}

fn kitty_image_data_from_ptr(ptr_out: *const u8, len: usize) -> Vec<u8> {
    if ptr_out.is_null() || len == 0 {
        return Vec::new();
    }
    unsafe { slice::from_raw_parts(ptr_out, len) }.to_vec()
}

// Hashes the full payload. Callers cache the result per image id and only
// recompute it when the image's transmit time changes.
fn kitty_image_fingerprint(
    ptr_out: *const u8,
    len: usize,
    image_width: u32,
    image_height: u32,
    format: KittyImageFormat,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    len.hash(&mut hasher);
    image_width.hash(&mut hasher);
    image_height.hash(&mut hasher);
    format.hash(&mut hasher);
    if ptr_out.is_null() || len == 0 {
        return hasher.finish();
    }

    let data = unsafe { slice::from_raw_parts(ptr_out, len) };
    data.hash(&mut hasher);
    hasher.finish()
}

pub struct RenderState {
    raw: ffi::GhosttyRenderState,
}

impl RenderState {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        // SAFETY: valid out pointer and null allocator use default allocator.
        unsafe {
            ffi::ghostty_render_state_new(ptr::null(), &mut raw).into_result()?;
        }
        Ok(Self { raw })
    }

    pub fn update(&mut self, terminal: &Terminal) -> Result<(), Error> {
        // SAFETY: both handles are valid for the duration of the call.
        unsafe { ffi::ghostty_render_state_update(self.raw, terminal.raw()).into_result() }
    }

    #[cfg(test)]
    pub fn cols(&self) -> Result<u16, Error> {
        self.get_u16(ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_COLS)
    }

    #[cfg(test)]
    pub fn rows(&self) -> Result<u16, Error> {
        self.get_u16(ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROWS)
    }

    pub fn dirty(&self) -> Result<Dirty, Error> {
        let mut out = ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        // SAFETY: out points to the matching enum storage for the requested data kind.
        unsafe {
            ffi::ghostty_render_state_get(
                self.raw,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_DIRTY,
                (&mut out as *mut ffi::GhosttyRenderStateDirty).cast(),
            )
            .into_result()?;
        }
        Ok(Dirty::from_raw(out))
    }

    pub fn cursor_visible(&self) -> Result<bool, Error> {
        self.get_bool(ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE)
    }

    pub fn cursor_blinking(&self) -> Result<bool, Error> {
        self.get_bool(ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING)
    }

    pub fn cursor_visual_style(&self) -> Result<CursorVisualStyle, Error> {
        let mut out: ffi::GhosttyRenderStateCursorVisualStyle = 0;
        // SAFETY: out points to the matching enum storage for the requested data kind.
        unsafe {
            ffi::ghostty_render_state_get(
                self.raw,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE,
                (&mut out as *mut ffi::GhosttyRenderStateCursorVisualStyle).cast(),
            )
            .into_result()?;
        }
        Ok(CursorVisualStyle::from_raw(out))
    }

    pub fn cursor_viewport(&self) -> Result<Option<CursorViewport>, Error> {
        if !self.get_bool(
            ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE,
        )? {
            return Ok(None);
        }
        Ok(Some(CursorViewport {
            x: self
                .get_u16(ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X)?,
            y: self
                .get_u16(ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y)?,
            wide_tail: self.get_bool(
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL,
            )?,
        }))
    }

    pub fn colors(&self) -> Result<RenderColors, Error> {
        let mut colors = ffi::GhosttyRenderStateColors {
            size: mem::size_of::<ffi::GhosttyRenderStateColors>(),
            ..Default::default()
        };
        unsafe {
            ffi::ghostty_render_state_colors_get(self.raw, &mut colors).into_result()?;
        }
        Ok(RenderColors {
            background: colors.background.into(),
            foreground: colors.foreground.into(),
            palette: colors.palette.map(Into::into),
        })
    }

    pub fn set_dirty(&mut self, dirty: Dirty) -> Result<(), Error> {
        let value = dirty.as_raw();
        // SAFETY: value pointer matches the expected option type.
        unsafe {
            ffi::ghostty_render_state_set(
                self.raw,
                ffi::GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPTION_DIRTY,
                (&value as *const ffi::GhosttyRenderStateDirty).cast(),
            )
            .into_result()
        }
    }

    pub fn populate_row_iterator<'a>(
        &'a self,
        iterator: &'a mut RowIterator,
    ) -> Result<RowIter<'a>, Error> {
        // SAFETY: iterator raw handle is valid and will not outlive self.
        unsafe {
            ffi::ghostty_render_state_get(
                self.raw,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&mut iterator.raw as *mut ffi::GhosttyRenderStateRowIterator).cast(),
            )
            .into_result()?;
        }
        Ok(RowIter {
            iterator,
            _state: PhantomData,
        })
    }

    fn get_u16(&self, data: ffi::GhosttyRenderStateData) -> Result<u16, Error> {
        let mut out = 0u16;
        // SAFETY: out points to a u16 matching the requested render-state data type.
        unsafe {
            ffi::ghostty_render_state_get(self.raw, data, (&mut out as *mut u16).cast())
                .into_result()?;
        }
        Ok(out)
    }

    fn get_bool(&self, data: ffi::GhosttyRenderStateData) -> Result<bool, Error> {
        let mut out = false;
        unsafe {
            ffi::ghostty_render_state_get(self.raw, data, (&mut out as *mut bool).cast())
                .into_result()?;
        }
        Ok(out)
    }
}

// SAFETY: these opaque handles are only used behind external synchronization in pane runtime.
unsafe impl Send for RenderState {}

impl Drop for RenderState {
    fn drop(&mut self) {
        // SAFETY: freeing a null or live handle is allowed by the C API.
        unsafe {
            ffi::ghostty_render_state_free(self.raw);
        }
    }
}

pub struct KeyEvent {
    raw: ffi::GhosttyKeyEvent,
}

impl KeyEvent {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        unsafe { ffi::ghostty_key_event_new(ptr::null(), &mut raw).into_result()? };
        Ok(Self { raw })
    }

    pub fn set_action(&mut self, action: ffi::GhosttyKeyAction) {
        unsafe { ffi::ghostty_key_event_set_action(self.raw, action) }
    }

    pub fn set_key(&mut self, key: u32) {
        unsafe { ffi::ghostty_key_event_set_key(self.raw, key) }
    }

    pub fn set_mods(&mut self, mods: u16) {
        unsafe { ffi::ghostty_key_event_set_mods(self.raw, mods) }
    }

    pub fn set_utf8(&mut self, text: &str) {
        unsafe {
            ffi::ghostty_key_event_set_utf8(self.raw, text.as_ptr().cast::<c_char>(), text.len())
        }
    }

    pub fn set_unshifted_codepoint(&mut self, codepoint: u32) {
        unsafe { ffi::ghostty_key_event_set_unshifted_codepoint(self.raw, codepoint) }
    }
}

impl Drop for KeyEvent {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_key_event_free(self.raw) }
    }
}

pub struct KeyEncoder {
    raw: ffi::GhosttyKeyEncoder,
}

impl KeyEncoder {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        unsafe { ffi::ghostty_key_encoder_new(ptr::null(), &mut raw).into_result()? };
        Ok(Self { raw })
    }

    pub fn set_from_terminal(&mut self, terminal: &Terminal) {
        unsafe { ffi::ghostty_key_encoder_setopt_from_terminal(self.raw, terminal.raw()) }
    }

    pub fn encode(&mut self, event: &KeyEvent) -> Result<Vec<u8>, Error> {
        encode_with_retry(|buf, len, out_len| unsafe {
            ffi::ghostty_key_encoder_encode(self.raw, event.raw, buf, len, out_len)
        })
    }
}

// SAFETY: the opaque encoder handle is only used behind external synchronization in pane runtime.
unsafe impl Send for KeyEncoder {}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_key_encoder_free(self.raw) }
    }
}

pub struct MouseEvent {
    raw: ffi::GhosttyMouseEvent,
}

impl MouseEvent {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        unsafe { ffi::ghostty_mouse_event_new(ptr::null(), &mut raw).into_result()? };
        Ok(Self { raw })
    }

    pub fn set_action(&mut self, action: ffi::GhosttyMouseAction) {
        unsafe { ffi::ghostty_mouse_event_set_action(self.raw, action) }
    }

    pub fn set_button(&mut self, button: ffi::GhosttyMouseButton) {
        unsafe { ffi::ghostty_mouse_event_set_button(self.raw, button) }
    }

    pub fn clear_button(&mut self) {
        unsafe { ffi::ghostty_mouse_event_clear_button(self.raw) }
    }

    pub fn set_mods(&mut self, mods: u16) {
        unsafe { ffi::ghostty_mouse_event_set_mods(self.raw, mods) }
    }

    pub fn set_position(&mut self, x: f32, y: f32) {
        unsafe {
            ffi::ghostty_mouse_event_set_position(self.raw, ffi::GhosttyMousePosition { x, y })
        }
    }
}

impl Drop for MouseEvent {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_mouse_event_free(self.raw) }
    }
}

pub struct MouseEncoder {
    raw: ffi::GhosttyMouseEncoder,
}

impl MouseEncoder {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        unsafe { ffi::ghostty_mouse_encoder_new(ptr::null(), &mut raw).into_result()? };
        Ok(Self { raw })
    }

    pub fn set_from_terminal(&mut self, terminal: &Terminal) {
        unsafe { ffi::ghostty_mouse_encoder_setopt_from_terminal(self.raw, terminal.raw()) }
    }

    pub fn set_size(
        &mut self,
        screen_width: u32,
        screen_height: u32,
        cell_width: u32,
        cell_height: u32,
    ) {
        let size = ffi::GhosttyMouseEncoderSize {
            size: std::mem::size_of::<ffi::GhosttyMouseEncoderSize>(),
            screen_width,
            screen_height,
            cell_width,
            cell_height,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        };
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.raw,
                ffi::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&size as *const ffi::GhosttyMouseEncoderSize).cast(),
            )
        }
    }

    pub fn set_format(&mut self, format: ffi::GhosttyMouseFormat) {
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.raw,
                ffi::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_FORMAT,
                (&format as *const ffi::GhosttyMouseFormat).cast(),
            )
        }
    }

    pub fn encode(&mut self, event: &MouseEvent) -> Result<Vec<u8>, Error> {
        encode_with_retry(|buf, len, out_len| unsafe {
            ffi::ghostty_mouse_encoder_encode(self.raw, event.raw, buf, len, out_len)
        })
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_mouse_encoder_free(self.raw) }
    }
}

fn encode_with_retry(
    mut encode: impl FnMut(*mut c_char, usize, *mut usize) -> ffi::GhosttyResult,
) -> Result<Vec<u8>, Error> {
    let mut required = 0usize;
    let result = encode(ptr::null_mut(), 0, &mut required);
    if result != ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        result.into_result()?;
    }
    let mut buffer = vec![0u8; required.max(16)];
    let mut written = 0usize;
    encode(
        buffer.as_mut_ptr().cast::<c_char>(),
        buffer.len(),
        &mut written,
    )
    .into_result()?;
    buffer.truncate(written);
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[path = "input_test.rs"]
    mod input;
    #[path = "kitty_test.rs"]
    mod kitty;
    #[path = "render_test.rs"]
    mod render;
    #[path = "terminal_test.rs"]
    mod terminal;
}
