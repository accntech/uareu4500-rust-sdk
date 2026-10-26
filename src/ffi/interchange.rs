// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WsqConfig {
    encoder: std::path::PathBuf,
    decoder: std::path::PathBuf,
    timeout_ms: Option<u64>,
    bitrate: Option<f64>,
}
impl WsqConfig {
    fn codec(&self) -> Result<crate::wsq::WsqCodec> {
        let timeout = self.timeout_ms.unwrap_or(30_000);
        if !(1..=300_000).contains(&timeout) {
            return Err(error(INVALID_ARGUMENT, "codec deadline out of range"));
        }
        Ok(crate::wsq::WsqCodec {
            encoder: self.encoder.clone(),
            decoder: self.decoder.clone(),
            timeout: Duration::from_millis(timeout),
        })
    }
}
/// External NBIS codec; output is a data handle containing a JSON byte array.
/// # Safety
/// config must contain length readable JSON bytes; output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_encode_wsq(
    handle: u64,
    config: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let config: WsqConfig = unsafe { json_input(config, length)? };
        let bytes = config
            .codec()?
            .encode(&image(handle)?.captured, config.bitrate.unwrap_or(0.75))
            .map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = data(&bytes)?;
        unsafe { write(output, id) }
    })
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WsqTargetConfig {
    encoder: std::path::PathBuf,
    decoder: std::path::PathBuf,
    timeout_ms: Option<u64>,
    target_bytes: usize,
    max_encodes: Option<usize>,
}
/// Measured WSQ size search; output owns bytes and explicit search diagnostics.
/// # Safety
/// config must contain length readable JSON bytes; output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_encode_wsq_target(
    handle: u64,
    config: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let config: WsqTargetConfig = unsafe { json_input(config, length)? };
        let max_encodes = config.max_encodes.unwrap_or(8);
        if !(1..=4 * 1024 * 1024).contains(&config.target_bytes) || !(1..=32).contains(&max_encodes)
        {
            return Err(error(
                INVALID_ARGUMENT,
                "WSQ target_bytes/max_encodes out of range",
            ));
        }
        let codec = WsqConfig {
            encoder: config.encoder,
            decoder: config.decoder,
            timeout_ms: config.timeout_ms,
            bitrate: None,
        }
        .codec()?;
        let result = codec
            .encode_to_target(
                &image(handle)?.captured,
                crate::wsq::WsqTargetOptions {
                    target_bytes: config.target_bytes,
                    max_encodes,
                    deadline: codec.timeout,
                },
            )
            .map_err(|e| mapped(IMAGE_ERROR, e))?;
        let (status, failure) = match result.status {
            crate::wsq::WsqTargetStatus::Met => ("met", None),
            crate::wsq::WsqTargetStatus::Unattainable => ("unattainable", None),
            crate::wsq::WsqTargetStatus::EncodeLimit => ("encode_limit", None),
            crate::wsq::WsqTargetStatus::Deadline => ("deadline", None),
            crate::wsq::WsqTargetStatus::CodecFailure(message) => ("codec_failure", Some(message)),
        };
        let id = data(
            &serde_json::json!({"bytes":result.bytes,"bitrate":result.bitrate,"encodes":result.encodes,"status":status,"failure":failure,"target_bytes":config.target_bytes}),
        )?;
        unsafe { write(output, id) }
    })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Jpeg2000Config {
    encoder: std::path::PathBuf,
    decoder: std::path::PathBuf,
    timeout_ms: Option<u64>,
}
impl Jpeg2000Config {
    fn codec(self) -> Result<crate::jpeg2000::Jpeg2000Codec> {
        let timeout = self.timeout_ms.unwrap_or(30_000);
        if !(1..=300_000).contains(&timeout) {
            return Err(error(INVALID_ARGUMENT, "codec deadline out of range"));
        }
        Ok(crate::jpeg2000::Jpeg2000Codec {
            encoder: self.encoder,
            decoder: self.decoder,
            timeout: Duration::from_millis(timeout),
        })
    }
}
/// Encode lossless grayscale J2K using explicitly supplied external OpenJPEG tools.
/// # Safety
/// config must contain length readable JSON bytes; output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_encode_jpeg2000(
    handle: u64,
    config: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let config: Jpeg2000Config = unsafe { json_input(config, length)? };
        let codec = config.codec()?;
        let bytes = codec
            .encode(&image(handle)?.captured)
            .map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = data(&bytes)?;
        unsafe { write(output, id) }
    })
}
/// Decode a J2K JSON byte-array data handle; raw codestreams carry no physical DPI.
/// # Safety
/// config must contain length readable JSON bytes; output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_decode_jpeg2000(
    handle: u64,
    config: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let config: Jpeg2000Config = unsafe { json_input(config, length)? };
        let codec = config.codec()?;
        let bytes: Vec<u8> = read_data(handle)?;
        let decoded = codec.decode(&bytes).map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = register_image(decoded)?;
        unsafe { write(output, id) }
    })
}

/// Decode a JSON-byte-array data handle using external NBIS, preserving its PPI declaration.
/// # Safety
/// config must contain length readable JSON bytes; output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_decode_wsq(
    handle: u64,
    config: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let config: WsqConfig = unsafe { json_input(config, length)? };
        let bytes: Vec<u8> = read_data(handle)?;
        let image = config
            .codec()?
            .decode(&bytes)
            .map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = register_image(image)?;
        unsafe { write(output, id) }
    })
}
/// Raw single-view ISO image record; config is explicit ImageRecordOptions JSON.
/// # Safety
/// config must contain length readable bytes and output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_encode_record(
    handle: u64,
    config: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let options = unsafe { json_input(config, length)? };
        let bytes = crate::image_record::encode(&image(handle)?.captured, &options)
            .map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = data(&bytes)?;
        unsafe { write(output, id) }
    })
}
/// Decode a raw ISO image record stored as JSON-byte-array data handle.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_decode_record(handle: u64, output: *mut u64) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let bytes: Vec<u8> = read_data(handle)?;
        let decoded = crate::image_record::decode(&bytes).map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = register_image(decoded.image)?;
        unsafe { write(output, id) }
    })
}

fn template_format(format: u32) -> Result<crate::template_io::TemplateFormat> {
    match format {
        0 => Ok(crate::template_io::TemplateFormat::Iso19794_2_2005),
        1 => Ok(crate::template_io::TemplateFormat::Ansi378_2004),
        _ => Err(error(INVALID_ARGUMENT, "unknown template format")),
    }
}
/// Encode a JSON MultiViewRecord data handle into a JSON byte-array data handle.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_template_encode_views(
    handle: u64,
    format: u32,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let format = template_format(format)?;
        let record = read_data(handle)?;
        let id = engine_data(crate::template_io::encode_views(&record, format))?;
        unsafe { write(output, id) }
    })
}
/// Decode a JSON byte-array data handle into a JSON MultiViewRecord data handle.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_template_decode_views(
    handle: u64,
    format: u32,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let format = template_format(format)?;
        let bytes: Vec<u8> = read_data(handle)?;
        let id = engine_data(crate::template_io::decode_views(&bytes, format))?;
        unsafe { write(output, id) }
    })
}
/// Encode an ExtendedMultiViewRecord JSON handle, preserving opaque extension blocks.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_template_encode_extended(
    handle: u64,
    format: u32,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let format = template_format(format)?;
        let record = read_data(handle)?;
        let id = engine_data(crate::template_io::encode_extended(&record, format))?;
        unsafe { write(output, id) }
    })
}
/// Decode a JSON byte-array handle into an owned ExtendedMultiViewRecord JSON handle.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_template_decode_extended(
    handle: u64,
    format: u32,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let format = template_format(format)?;
        let bytes: Vec<u8> = read_data(handle)?;
        let id = engine_data(crate::template_io::decode_extended(&bytes, format))?;
        unsafe { write(output, id) }
    })
}

/// Encode explicit raw8 ANSI381 declarations into an owned JSON byte-array handle.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_encode_ansi381(handle: u64, output: *mut u64) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let record = read_data(handle)?;
        let id = engine_data(crate::ansi381::encode(&record))?;
        unsafe { write(output, id) }
    })
}
/// Decode an ANSI381 JSON byte-array handle into owned record declaration JSON.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_decode_ansi381(handle: u64, output: *mut u64) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let bytes: Vec<u8> = read_data(handle)?;
        let id = engine_data(crate::ansi381::decode(&bytes))?;
        unsafe { write(output, id) }
    })
}
/// Extract a zero-based raw view, retaining declared header scale.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_ansi381_view(
    handle: u64,
    view: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let record: crate::ansi381::Ansi381Record = read_data(handle)?;
        let captured = record.raw_image(view).map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = register_image(captured)?;
        unsafe { write(output, id) }
    })
}

/// Encode JSON MultiImageRecord declarations/payloads into a JSON byte-array handle.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_encode_images(handle: u64, output: *mut u64) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let record = read_data(handle)?;
        let id = engine_data(crate::image_record::encode_images(&record))?;
        unsafe { write(output, id) }
    })
}
/// Decode a JSON byte-array handle into JSON MultiImageRecord declarations/payloads.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_decode_images(handle: u64, output: *mut u64) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let bytes: Vec<u8> = read_data(handle)?;
        let id = engine_data(crate::image_record::decode_images(&bytes))?;
        unsafe { write(output, id) }
    })
}

/// Build a host correction profile from explicit dark and bright reference images.
/// # Safety
/// output must point to writable u64 storage. The returned JSON data handle is owned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_flat_field_profile(
    dark: u64,
    bright: u64,
    minimum_response: f64,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let dark = image(dark)?;
        let bright = image(bright)?;
        let profile = crate::processing::FlatFieldProfile::from_references(
            &[crate::processing::FlatFieldReference {
                label: "dark",
                image: &dark.captured,
            }],
            &[crate::processing::FlatFieldReference {
                label: "bright",
                image: &bright.captured,
            }],
            crate::processing::FlatFieldConfig {
                minimum_response,
                maximum_response: 255.0,
            },
        )
        .map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = data(&profile)?;
        unsafe { write(output, id) }
    })
}
/// Apply a validated JSON correction profile to a borrowed image, returning a new image.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_apply_flat_field(
    handle: u64,
    profile: u64,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let profile: crate::processing::FlatFieldProfile = read_data(profile)?;
        let corrected = profile
            .apply(&image(handle)?.captured)
            .map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = register_image(corrected)?;
        unsafe { write(output, id) }
    })
}
