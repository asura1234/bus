//! Kitty image descriptors, storage queries, data copying and fingerprint cache.
use super::callbacks::install_png_decoder_once;
use super::consts::{APC_MAX_BYTES, APC_MAX_BYTES_KITTY, KITTY_IMAGE_STORAGE_LIMIT_BYTES};
use super::kitty_placement::KittyPlacementIteratorGuard;
use super::{ffi, Error, GhosttyResultExt, Terminal};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::{ptr, slice};

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
pub(super) struct KittyImageFingerprintEntry {
    pub(super) generation: u64,
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

impl Terminal {
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

    /// Copy image bytes only when the caller requests this exact descriptor.
    /// The image handle and its data remain borrowed from the terminal for this call.
    pub(super) fn kitty_image_descriptor_and_data<F>(
        &self,
        image: ffi::GhosttyKittyGraphicsImage,
        image_id: u32,
        placement_id: u32,
        image_width: u32,
        image_height: u32,
        format: KittyImageFormat,
        needs_data: &mut F,
    ) -> Result<(KittyImageDescriptor, Vec<u8>), Error>
    where
        F: FnMut(KittyImageDescriptor) -> bool,
    {
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
        Ok((descriptor, data))
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

pub(super) fn kitty_image_u32(
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

pub(super) fn kitty_image_format(
    image: ffi::GhosttyKittyGraphicsImage,
) -> Result<KittyImageFormat, Error> {
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

pub(super) fn kitty_image_compression(
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
pub(super) fn kitty_image_fingerprint(
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
