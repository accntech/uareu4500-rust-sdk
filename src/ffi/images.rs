// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub bits_per_pixel: u32,
}

/// Copy raw grayscale pixels and explicit metadata into a new image.
/// # Safety
/// All byte ranges must be valid, and output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_from_pixels(
    width: u32,
    height: u32,
    pixels: *const u8,
    length: usize,
    metadata: *const u8,
    metadata_len: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        if metadata_len > 16_384
            || width == 0
            || height == 0
            || length > 16 * 1024 * 1024
            || pixels.is_null()
            || (width as usize).checked_mul(height as usize) != Some(length)
        {
            return Err(error(INVALID_ARGUMENT, "invalid pixel geometry"));
        }
        let image = crate::frame::Image {
            width: width as usize,
            height: height as usize,
            pixels: unsafe { std::slice::from_raw_parts(pixels, length) }.to_vec(),
        };
        let metadata = if metadata_len == 0 {
            crate::metadata::CaptureMetadata::default()
        } else {
            unsafe { json_input(metadata, metadata_len)? }
        };
        let captured = CapturedImage { image, metadata };
        captured
            .metadata
            .validate(&captured.image)
            .map_err(|e| mapped(INVALID_ARGUMENT, e))?;
        let id = register_image(captured)?;
        unsafe { write(output, id) }
    })
}

/// # Safety
/// `output` must hold `output_size` writable bytes, at least sizeof(ImageInfo), and be aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_get_info(
    handle: u64,
    output: *mut ImageInfo,
    output_size: usize,
) -> u32 {
    boundary(|| {
        if output_size < std::mem::size_of::<ImageInfo>() {
            return Err(error(INVALID_ARGUMENT, "image-info structure is too small"));
        }
        valid_output(output)?;
        let image = image(handle)?;
        let pixels = &image.captured.image;
        let width = u32::try_from(pixels.width)
            .map_err(|_| error(IMAGE_ERROR, "image width exceeds C ABI"))?;
        let height = u32::try_from(pixels.height)
            .map_err(|_| error(IMAGE_ERROR, "image height exceeds C ABI"))?;
        unsafe {
            write(
                output,
                ImageInfo {
                    width,
                    height,
                    stride_bytes: width,
                    bits_per_pixel: 8,
                },
            )
        }
    })
}

/// Copy row-major 8-bit grayscale pixels. The required size excludes any terminator.
/// # Safety
/// Buffer/required pointers follow the contract of `uareu_last_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_get_pixels(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let image = image(handle)?;
        unsafe {
            copy_buffer(
                &image.captured.image.pixels,
                false,
                buffer,
                capacity,
                required,
            )
        }
    })
}

/// UTF-8 JSON metadata and image diagnostics. No NFIQ engine is run.
/// # Safety
/// Buffer/required pointers follow the contract of `uareu_last_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_get_report_json(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let image = image(handle)?;
        unsafe { copy_buffer(&image.report, true, buffer, capacity, required) }
    })
}

/// Save a new PNG with metadata; existing paths are never overwritten.
/// # Safety
/// `path` must point to `path_length` readable UTF-8 bytes; no NUL is included.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_save_png(
    handle: u64,
    path: *const u8,
    path_length: usize,
) -> u32 {
    boundary(|| {
        let path = unsafe { utf8_path(path, path_length)? };
        image(handle)?
            .captured
            .save_png(path)
            .map_err(|cause| mapped(IMAGE_ERROR, cause))
    })
}

/// Inspect an existing PNG without USB access; embedded metadata remains a declaration.
/// # Safety
/// `path` must point to `path_length` readable UTF-8 bytes, and `output` to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_load_png(
    path: *const u8,
    path_length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let path = unsafe { utf8_path(path, path_length)? };
        let captured = CapturedImage::load_png(path).map_err(|cause| mapped(IMAGE_ERROR, cause))?;
        let id = register_image(captured)?;
        unsafe { write(output, id) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn uareu_image_free(handle: u64) -> u32 {
    boundary(|| {
        release(handle, |object| matches!(object, Object::Image(_)))?;
        Ok(())
    })
}
