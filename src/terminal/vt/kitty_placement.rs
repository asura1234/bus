//! Ordinary placements and Unicode placeholder run/geometry resolution.
use super::consts::{
    KITTY_PLACEMENT_DATA_COLUMNS, KITTY_PLACEMENT_DATA_IS_VIRTUAL, KITTY_PLACEMENT_DATA_ROWS,
};
use super::kitty::{kitty_image_compression, kitty_image_format, kitty_image_u32};
use super::{
    ffi, CellColor, CellStyle, Error, GhosttyResultExt, KittyImageDescriptor, KittyImagePlacement,
    KittyPlacementRenderInfo, Terminal, KITTY_UNICODE_PLACEHOLDER,
};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::{mem, ptr};

static KITTY_PLACEHOLDER_DIACRITICS: OnceLock<HashMap<u32, u32>> = OnceLock::new();

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

impl Terminal {
    pub(super) fn kitty_image_placement<F>(
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
        let (descriptor, data) = self.kitty_image_descriptor_and_data(
            image,
            image_id,
            placement_id,
            image_width,
            image_height,
            format,
            needs_data,
        )?;
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
            data_len: descriptor.data_len,
            data_fingerprint: descriptor.data_fingerprint,
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

    pub(super) fn kitty_virtual_image_placements<F>(
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
        let runs = self.kitty_virtual_runs(viewport_cols, viewport_rows)?;

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
            let (descriptor, data) = self.kitty_image_descriptor_and_data(
                image,
                image_id,
                placement_id,
                image_width,
                image_height,
                format,
                needs_data,
            )?;
            placements.push(KittyImagePlacement {
                image_id,
                placement_id,
                z: spec.z,
                x_offset: geometry.x_offset,
                y_offset: geometry.y_offset,
                image_width,
                image_height,
                format,
                data_len: descriptor.data_len,
                data_fingerprint: descriptor.data_fingerprint,
                data,
                render: geometry.render,
            });
        }

        Ok(placements)
    }

    fn kitty_virtual_runs(
        &self,
        viewport_cols: u16,
        viewport_rows: u16,
    ) -> Result<Vec<KittyVirtualRun>, Error> {
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

        Ok(runs)
    }
}

pub(super) struct KittyPlacementIteratorGuard {
    pub(super) raw: ffi::GhosttyKittyGraphicsPlacementIterator,
}

impl KittyPlacementIteratorGuard {
    pub(super) fn new(graphics: ffi::GhosttyKittyGraphics) -> Result<Self, Error> {
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
        // Reuse Ghostty's vendored table so Bus decodes the same placeholder
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

    if run.col >= grid_cols || run.row >= grid_rows || image_width == 0 || image_height == 0 {
        return None;
    }
    let visible_cols = run.width.min(grid_cols.saturating_sub(run.col)).max(1);

    // Fit the whole image into the placeholder grid preserving its aspect
    // ratio and center it, as the Kitty spec and libghostty's renderer do.
    let grid_width = u64::from(grid_cols) * u64::from(cell_width);
    let grid_height = u64::from(grid_rows) * u64::from(cell_height);
    let (fit_width, fit_height) =
        if u64::from(image_width) * grid_height > u64::from(image_height) * grid_width {
            let height = u64::from(image_height) * grid_width / u64::from(image_width);
            (grid_width, height.max(1))
        } else {
            let width = u64::from(image_width) * grid_height / u64::from(image_height);
            (width.max(1), grid_height)
        };
    let fit_x = (grid_width - fit_width) / 2;
    let fit_y = (grid_height - fit_height) / 2;

    // Intersect this run's cells with the fitted image rectangle.
    let run_x = u64::from(run.col) * u64::from(cell_width);
    let run_y = u64::from(run.row) * u64::from(cell_height);
    let left = run_x.max(fit_x);
    let right = (run_x + u64::from(visible_cols) * u64::from(cell_width)).min(fit_x + fit_width);
    let top = run_y.max(fit_y);
    let bottom = (run_y + u64::from(cell_height)).min(fit_y + fit_height);
    if left >= right || top >= bottom {
        return None;
    }

    let source_x = scale_u64(left - fit_x, image_width, fit_width);
    let source_y = scale_u64(top - fit_y, image_height, fit_height);
    let source_width = scale_u64(right - fit_x, image_width, fit_width)
        .saturating_sub(source_x)
        .max(1)
        .min(image_width.saturating_sub(source_x));
    let source_height = scale_u64(bottom - fit_y, image_height, fit_height)
        .saturating_sub(source_y)
        .max(1)
        .min(image_height.saturating_sub(source_y));
    if source_width == 0 || source_height == 0 {
        return None;
    }

    // Padding may cover whole leading cells; anchor at the first cell the
    // image touches so the pixel offsets stay within one cell.
    let skipped_cols = (left - run_x) / u64::from(cell_width);
    let x_offset = ((left - run_x) % u64::from(cell_width)) as u32;
    let y_offset = (top - run_y) as u32;
    let pixel_width = (right - left) as u32;
    let pixel_height = (bottom - top) as u32;

    Some(KittyVirtualPlacementGeometry {
        x_offset,
        y_offset,
        render: KittyPlacementRenderInfo {
            pixel_width,
            pixel_height,
            grid_cols: (x_offset + pixel_width).div_ceil(cell_width),
            grid_rows: 1,
            viewport_col: i32::from(run.x) + skipped_cols as i32,
            viewport_row: i32::from(run.y),
            source_x,
            source_y,
            source_width,
            source_height,
        },
    })
}

fn scale_u64(value: u64, source: u32, dest: u64) -> u32 {
    (value.saturating_mul(u64::from(source)) / dest.max(1)).min(u64::from(u32::MAX)) as u32
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
