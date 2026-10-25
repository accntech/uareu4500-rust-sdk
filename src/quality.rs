// SPDX-License-Identifier: GPL-3.0-or-later
//! Advisory diagnostics and optional NFIQ 2 evaluation, independent of USB I/O.
mod nfiq2;
pub use nfiq2::Nfiq2;

use crate::metadata::{CaptureMetadata, CapturedImage, Region};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize)]
pub struct ImageDiagnostics {
    pub canvas_pixels: usize,
    pub analyzed_pixels: usize,
    pub known_padding_fraction: Option<f64>,
    pub minimum: u8,
    pub maximum: u8,
    pub mean: f64,
    pub standard_deviation: f64,
    pub zero_fraction: f64,
    pub full_scale_fraction: f64,
}

/// Population statistics over the descriptor-populated region when known.
/// These are image properties, not fingerprint detection or acceptance scores.
pub fn diagnostics(captured: &CapturedImage) -> Result<ImageDiagnostics> {
    captured.metadata.validate(&captured.image)?;
    let image = &captured.image;
    let region = analysis_region(captured);
    let mut histogram = [0usize; 256];
    for y in region.y..region.y + region.height {
        for &pixel in
            &image.pixels[y * image.width + region.x..y * image.width + region.x + region.width]
        {
            histogram[usize::from(pixel)] += 1;
        }
    }
    let count = region.width * region.height;
    let mean = histogram
        .iter()
        .enumerate()
        .map(|(value, n)| value as f64 * *n as f64)
        .sum::<f64>()
        / count as f64;
    let variance = histogram
        .iter()
        .enumerate()
        .map(|(value, n)| (value as f64 - mean).powi(2) * *n as f64)
        .sum::<f64>()
        / count as f64;
    Ok(ImageDiagnostics {
        canvas_pixels: image.pixels.len(),
        analyzed_pixels: count,
        known_padding_fraction: captured
            .metadata
            .valid_region
            .map(|_| 1.0 - count as f64 / image.pixels.len() as f64),
        minimum: histogram.iter().position(|n| *n > 0).unwrap_or(0) as u8,
        maximum: histogram.iter().rposition(|n| *n > 0).unwrap_or(0) as u8,
        mean,
        standard_deviation: variance.sqrt(),
        zero_fraction: histogram[0] as f64 / count as f64,
        full_scale_fraction: histogram[255] as f64 / count as f64,
    })
}

pub(crate) fn analysis_region(captured: &CapturedImage) -> Region {
    captured.metadata.valid_region.unwrap_or(Region {
        x: 0,
        y: 0,
        width: captured.image.width,
        height: captured.image.height,
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineIdentity {
    pub name: &'static str,
    pub version: String,
    pub executable_sha256: String,
    pub model_source: &'static str,
    pub model_info_sha256: Option<String>,
    pub model_data_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QualityOutcome {
    NotEvaluated,
    UnsupportedInput {
        reason: String,
    },
    EngineUnavailable {
        reason: String,
    },
    EvaluationFailed {
        reason: String,
    },
    Scored {
        engine: EngineIdentity,
        /// NFIQ 2 unified score, 0..=100, higher is better. Not legacy NFIQ.
        score: u8,
        input_region: Region,
        evaluation_ppi: u32,
        /// Upstream actionable measure names and values; not liveness claims.
        actionable_feedback: BTreeMap<String, f64>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct QualityReport {
    pub schema_version: u32,
    pub width: usize,
    pub height: usize,
    pub metadata: CaptureMetadata,
    pub diagnostics: ImageDiagnostics,
    pub quality: QualityOutcome,
    pub feedback: crate::processing::ImageFeedback,
}

pub fn assess(captured: &CapturedImage, engine: Option<&Nfiq2>) -> Result<QualityReport> {
    let diagnostics = diagnostics(captured)?;
    Ok(QualityReport {
        schema_version: 1,
        width: captured.image.width,
        height: captured.image.height,
        metadata: captured.metadata.clone(),
        diagnostics,
        quality: engine.map_or(QualityOutcome::NotEvaluated, |engine| {
            engine.assess(captured)
        }),
        feedback: crate::processing::feedback(captured)?,
    })
}
