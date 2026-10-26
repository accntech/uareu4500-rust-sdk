// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AssessmentOptions {
    engine_path: Option<std::path::PathBuf>,
    model_path: Option<std::path::PathBuf>,
    timeout_ms: Option<u64>,
    ppi: Option<f64>,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyAssessmentOptions {
    engine_path: std::path::PathBuf,
    timeout_ms: Option<u64>,
    ppi: Option<f64>,
}
/// Return a separate legacy NFIQ 1 outcome; class 1 is best and 5 is worst.
/// # Safety
/// config must contain length readable JSON bytes; output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_assess_nfiq1(
    handle: u64,
    config: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let options: LegacyAssessmentOptions = unsafe { json_input(config, length)? };
        let timeout = options.timeout_ms.unwrap_or(10_000);
        if !(1..=300_000).contains(&timeout) {
            return Err(error(INVALID_ARGUMENT, "engine deadline out of range"));
        }
        let mut captured = image(handle)?.captured.clone();
        if let Some(ppi) = options.ppi {
            captured.metadata.resolution = crate::metadata::Resolution::declared(ppi, ppi)
                .map_err(|e| mapped(INVALID_ARGUMENT, e))?;
        }
        let outcome = quality::Nfiq1 {
            executable: options.engine_path,
            timeout: Duration::from_millis(timeout),
        }
        .assess(&captured);
        let id = data(&outcome)?;
        unsafe { write(output, id) }
    })
}

/// Evaluate once and return an immutable report handle (so probing never reruns an engine).
/// # Safety
/// JSON must be readable for length bytes and output writable for a u64.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_assess(
    handle: u64,
    config: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let options: AssessmentOptions = unsafe { json_input(config, length)? };
        let mut captured = image(handle)?.captured.clone();
        if let Some(ppi) = options.ppi {
            captured.metadata.resolution = crate::metadata::Resolution::declared(ppi, ppi)
                .map_err(|e| mapped(INVALID_ARGUMENT, e))?;
        }
        let timeout = options.timeout_ms.unwrap_or(10_000);
        if !(1..=300_000).contains(&timeout) {
            return Err(error(INVALID_ARGUMENT, "engine timeout out of range"));
        }
        let engine = options.engine_path.map(|executable| quality::Nfiq2 {
            executable,
            model_info: options.model_path,
            timeout: Duration::from_millis(timeout),
        });
        let report =
            quality::assess(&captured, engine.as_ref()).map_err(|e| mapped(IMAGE_ERROR, e))?;
        let handle = data(&report)?;
        unsafe { write(output, handle) }
    })
}
/// Assess one declared raw image-record view; resolution is never overridden.
/// # Safety
/// config must contain length readable JSON bytes; output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_assess_record_view(
    handle: u64,
    view: usize,
    config: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let options: AssessmentOptions = unsafe { json_input(config, length)? };
        let timeout = options.timeout_ms.unwrap_or(10_000);
        if options.ppi.is_some() || !(1..=300_000).contains(&timeout) {
            return Err(error(
                INVALID_ARGUMENT,
                "record assessment preserves declared scale; timeout must be 1..300000",
            ));
        }
        let record: crate::image_record::MultiImageRecord = read_data(handle)?;
        let outcome = if let Some(executable) = options.engine_path {
            quality::Nfiq2 {
                executable,
                model_info: options.model_path,
                timeout: Duration::from_millis(timeout),
            }
            .assess_record_view(&record, view)
        } else {
            match record.raw_image(view) {
                Ok(_) => quality::QualityOutcome::NotEvaluated,
                Err(error) => quality::QualityOutcome::UnsupportedInput {
                    reason: format!("{error:#}"),
                },
            }
        };
        let id = data(&outcome)?;
        unsafe { write(output, id) }
    })
}

/// Explicit display enhancement creates a new image and preserves the original.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_enhance(handle: u64, output: *mut u64) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let enhanced = crate::processing::enhance(&image(handle)?.captured)
            .map_err(|e| mapped(IMAGE_ERROR, e))?;
        let id = register_image(enhanced)?;
        unsafe { write(output, id) }
    })
}
