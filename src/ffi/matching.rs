// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecognitionConfig {
    executable: std::path::PathBuf,
    timeout_ms: Option<u64>,
    ppi: Option<f64>,
}
#[derive(serde::Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum RecognitionRequest {
    ExportTemplate {
        template: u64,
        format: crate::template_io::TemplateFormat,
        quality: u8,
        impression: u8,
    },
    ImportTemplate {
        data: u64,
        format: crate::template_io::TemplateFormat,
    },
    Extract {
        image: u64,
    },
    Compare {
        probe: u64,
        candidate: u64,
        threshold: f64,
    },
    Identify {
        probe: u64,
        candidates: Vec<u64>,
        threshold: f64,
        maximum: usize,
    },
    Enroll {
        samples: Vec<u64>,
        threshold: f64,
    },
    VerifyEnrollment {
        probe: u64,
        enrollment: u64,
        threshold: f64,
    },
}
/// Optional recognition backend. JSON operations: extract/compare/identify/enroll/
/// verify_enrollment. Input handles are borrowed; output is an owned data handle.
/// # Safety
/// Both JSON ranges must be readable; output must be writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_recognition_execute(
    config: *const u8,
    config_len: usize,
    request: *const u8,
    request_len: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let config: RecognitionConfig = unsafe { json_input(config, config_len)? };
        let request: RecognitionRequest = unsafe { json_input(request, request_len)? };
        let timeout = config.timeout_ms.unwrap_or(30_000);
        if !(1..=300_000).contains(&timeout) {
            return Err(error(INVALID_ARGUMENT, "recognition timeout out of range"));
        }
        let engine = crate::recognition::RecognitionEngine {
            executable: config.executable,
            timeout: Duration::from_millis(timeout),
        };
        let id = match request {
            RecognitionRequest::ExportTemplate {
                template,
                format,
                quality,
                impression,
            } => {
                let template = read_data(template)?;
                engine_data(engine.export_template(&template, format, quality, impression))
            }
            RecognitionRequest::ImportTemplate {
                data: handle,
                format,
            } => {
                let bytes: Vec<u8> = read_data(handle)?;
                engine_data(engine.import_template(&bytes, format))
            }
            RecognitionRequest::Extract { image: handle } => {
                let mut captured = image(handle)?.captured.clone();
                if let Some(ppi) = config.ppi {
                    captured.metadata.resolution = crate::metadata::Resolution::declared(ppi, ppi)
                        .map_err(|e| mapped(INVALID_ARGUMENT, e))?;
                }
                engine_data(engine.extract(&captured))
            }
            RecognitionRequest::Compare {
                probe,
                candidate,
                threshold,
            } => {
                let probe = read_data(probe)?;
                let candidate = read_data(candidate)?;
                engine_data(engine.compare(&probe, &candidate, threshold))
            }
            RecognitionRequest::Identify {
                probe,
                candidates,
                threshold,
                maximum,
            } => {
                if candidates.len() > 1000 {
                    return Err(error(INVALID_ARGUMENT, "too many candidates"));
                }
                let probe = read_data(probe)?;
                let candidates = candidates
                    .into_iter()
                    .map(read_data)
                    .collect::<Result<Vec<_>>>()?;
                engine_data(engine.identify(&probe, &candidates, threshold, maximum))
            }
            RecognitionRequest::Enroll { samples, threshold } => {
                if !(2..=10).contains(&samples.len()) {
                    return Err(error(INVALID_ARGUMENT, "enrollment requires 2..10 samples"));
                }
                let samples = samples
                    .into_iter()
                    .map(read_data)
                    .collect::<Result<Vec<_>>>()?;
                engine_data(engine.enroll(&samples, threshold))
            }
            RecognitionRequest::VerifyEnrollment {
                probe,
                enrollment,
                threshold,
            } => {
                let probe = read_data(probe)?;
                let enrollment = read_data(enrollment)?;
                engine_data(engine.verify_enrollment(&probe, &enrollment, threshold))
            }
        }?;
        unsafe { write(output, id) }
    })
}
