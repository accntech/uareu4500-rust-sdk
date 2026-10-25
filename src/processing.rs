// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit display processing; never substitutes for vendor correction/PIV.
use crate::{
    metadata::{CapturedImage, ProcessingStep, Region},
    quality::{analysis_region, diagnostics},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize)]
pub struct ImageFeedback {
    pub method: &'static str,
    pub block_size: usize,
    pub contrast_threshold: f64,
    pub textured_fraction: f64,
    pub textured_bounds: Option<Region>,
    pub warnings: Vec<&'static str>,
}
pub fn feedback(captured: &CapturedImage) -> Result<ImageFeedback> {
    let stats = diagnostics(captured)?;
    let region = analysis_region(captured);
    let block = 16;
    let mut textured = 0;
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for y in (region.y..region.y + region.height).step_by(block) {
        for x in (region.x..region.x + region.width).step_by(block) {
            let right = (x + block).min(region.x + region.width);
            let bottom = (y + block).min(region.y + region.height);
            let count = (right - x) * (bottom - y);
            let (mut sum, mut squares) = (0f64, 0f64);
            for row in y..bottom {
                for &p in &captured.image.pixels
                    [row * captured.image.width + x..row * captured.image.width + right]
                {
                    sum += p as f64;
                    squares += (p as f64).powi(2);
                }
            }
            if (squares / count as f64 - (sum / count as f64).powi(2))
                .max(0.0)
                .sqrt()
                >= 12.0
            {
                textured += count;
                bounds = Some(bounds.map_or((x, y, right, bottom), |(l, t, r, b)| {
                    (l.min(x), t.min(y), r.max(right), b.max(bottom))
                }));
            }
        }
    }
    let mut warnings = Vec::new();
    if stats.standard_deviation < 10.0 {
        warnings.push("low_global_contrast");
    }
    if stats.mean > 245.0 {
        warnings.push("mostly_white");
    }
    if stats.mean < 10.0 {
        warnings.push("mostly_black");
    }
    if textured == 0 {
        warnings.push("no_textured_blocks");
    }
    if bounds.is_some_and(|(l, t, r, b)| {
        l == region.x
            || t == region.y
            || r == region.x + region.width
            || b == region.y + region.height
    }) {
        warnings.push("textured_content_touches_boundary");
    }
    Ok(ImageFeedback {
        method: "advisory_block_contrast_v1_not_biometric_acceptance",
        block_size: block,
        contrast_threshold: 12.0,
        textured_fraction: textured as f64 / stats.analyzed_pixels as f64,
        textured_bounds: bounds.map(|(x, y, r, b)| Region {
            x,
            y,
            width: r - x,
            height: b - y,
        }),
        warnings,
    })
}
/// Percentile stretch of the declared valid region. Returns a new image.
pub fn enhance(captured: &CapturedImage) -> Result<CapturedImage> {
    captured.metadata.validate(&captured.image)?;
    let region = analysis_region(captured);
    let mut histogram = [0usize; 256];
    for y in region.y..region.y + region.height {
        for &p in &captured.image.pixels[y * captured.image.width + region.x
            ..y * captured.image.width + region.x + region.width]
        {
            histogram[p as usize] += 1;
        }
    }
    let total = region.width * region.height;
    let quantile = |target| {
        let mut n = 0;
        histogram
            .iter()
            .position(|count| {
                n += count;
                n > target
            })
            .unwrap_or(255) as u8
    };
    let low = quantile(total / 100);
    let high = quantile(total * 99 / 100);
    ensure!(
        high > low,
        "image has insufficient intensity range for enhancement"
    );
    let mut output = captured.clone();
    for y in region.y..region.y + region.height {
        for p in &mut output.image.pixels[y * captured.image.width + region.x
            ..y * captured.image.width + region.x + region.width]
        {
            *p =
                (((*p as i32 - low as i32) * 255) / (high as i32 - low as i32)).clamp(0, 255) as u8;
        }
    }
    output
        .metadata
        .processing
        .push(ProcessingStep::ContrastStretch {
            lower: low,
            upper: high,
        });
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Image;
    #[test]
    fn advisory_feedback_and_enhancement_do_not_establish_resolution() {
        let uniform = CapturedImage::new(Image {
            width: 32,
            height: 32,
            pixels: vec![250; 1024],
        });
        let f = feedback(&uniform).unwrap();
        assert_eq!(f.textured_fraction, 0.0);
        assert!(f.warnings.contains(&"low_global_contrast"));
        assert!(enhance(&uniform).is_err());
        let input = CapturedImage::new(Image {
            width: 32,
            height: 32,
            pixels: (0..1024).map(|i| 200 + (i % 50) as u8).collect(),
        });
        let output = enhance(&input).unwrap();
        assert_eq!(output.metadata.resolution, input.metadata.resolution);
        assert_eq!(*output.image.pixels.iter().min().unwrap(), 0);
        assert_eq!(*output.image.pixels.iter().max().unwrap(), 255);
        assert!(input.image.pixels.iter().all(|p| *p >= 200));
    }
    #[test]
    fn nonfinite_flat_field_coefficients_are_rejected() {
        let dark = CapturedImage::new(Image {
            width: 1,
            height: 1,
            pixels: vec![0],
        });
        let bright = CapturedImage::new(Image {
            width: 1,
            height: 1,
            pixels: vec![255],
        });
        let mut profile = FlatFieldProfile::from_references(
            &[FlatFieldReference {
                label: "dark",
                image: &dark,
            }],
            &[FlatFieldReference {
                label: "bright",
                image: &bright,
            }],
            FlatFieldConfig::default(),
        )
        .unwrap();
        profile.gains[0] = f64::INFINITY;
        assert!(profile.apply(&dark).is_err());
        profile.gains[0] = 1.0;
        profile.offsets[0] = f64::NAN;
        assert!(profile.apply(&dark).is_err());
    }
}

/// Caller-declared dark/bright reference. A label identifies acquisition context;
/// neither a label nor this API establishes a physically validated calibration.
#[derive(Clone, Copy, Debug)]
pub struct FlatFieldReference<'a> {
    pub label: &'a str,
    pub image: &'a CapturedImage,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FlatFieldConfig {
    /// Minimum accepted per-pixel difference of mean bright and dark (DN).
    pub minimum_response: f64,
    /// Maximum accepted per-pixel difference (DN), at most 255.
    pub maximum_response: f64,
}
impl Default for FlatFieldConfig {
    fn default() -> Self {
        Self {
            minimum_response: 8.0,
            maximum_response: 255.0,
        }
    }
}
impl FlatFieldConfig {
    fn validate(self) -> Result<()> {
        ensure!(
            self.minimum_response.is_finite()
                && self.maximum_response.is_finite()
                && self.minimum_response > 0.0
                && self.minimum_response <= self.maximum_response
                && self.maximum_response <= 255.0,
            "invalid flat-field response bounds"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FlatFieldSource {
    label: String,
    sha256: [u8; 32],
}
/// Serializable host-side linear correction profile. Loaded profiles are fully
/// validated on apply; a SHA-256 binds coefficients, labels, reference hashes and
/// configuration. This digest detects changes, not maliciously forged profiles.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlatFieldProfile {
    width: usize,
    height: usize,
    config: FlatFieldConfig,
    dark_sources: Vec<FlatFieldSource>,
    bright_sources: Vec<FlatFieldSource>,
    offsets: Vec<f64>,
    gains: Vec<f64>,
    sha256: [u8; 32],
}
impl FlatFieldProfile {
    const MAX_PIXELS: usize = 16 * 1024 * 1024;
    const MAX_REFERENCES: usize = 64;

    fn source_sums(
        references: &[FlatFieldReference<'_>],
        width: usize,
        height: usize,
    ) -> Result<(Vec<u32>, Vec<FlatFieldSource>)> {
        ensure!(
            !references.is_empty() && references.len() <= Self::MAX_REFERENCES,
            "flat-field requires 1..64 references of each type"
        );
        // Validate every reference before allocating accumulators.
        for reference in references {
            ensure!(
                !reference.label.trim().is_empty() && reference.label.len() <= 256,
                "flat-field reference label must be 1..256 bytes"
            );
            reference.image.metadata.validate(&reference.image.image)?;
            ensure!(
                reference.image.image.width == width && reference.image.image.height == height,
                "conflicting flat-field reference dimensions"
            );
        }
        let mut sums = vec![0; width * height];
        let mut sources = Vec::with_capacity(references.len());
        for reference in references {
            for (sum, &pixel) in sums.iter_mut().zip(&reference.image.image.pixels) {
                *sum += u32::from(pixel);
            }
            let mut hash = Sha256::new();
            hash.update(b"uareu.flat-field.reference.v1");
            hash.update((width as u64).to_le_bytes());
            hash.update((height as u64).to_le_bytes());
            hash.update(&reference.image.image.pixels);
            hash.update(serde_json::to_vec(&reference.image.metadata)?);
            sources.push(FlatFieldSource {
                label: reference.label.to_owned(),
                sha256: hash.finalize().into(),
            });
        }
        Ok((sums, sources))
    }

    pub fn from_references(
        dark: &[FlatFieldReference<'_>],
        bright: &[FlatFieldReference<'_>],
        config: FlatFieldConfig,
    ) -> Result<Self> {
        config.validate()?;
        let first = dark
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing dark references"))?;
        let (width, height) = (first.image.image.width, first.image.image.height);
        Self::validate_geometry(width, height)?;
        let (dark_sums, dark_sources) = Self::source_sums(dark, width, height)?;
        let (bright_sums, bright_sources) = Self::source_sums(bright, width, height)?;
        let mut offsets = Vec::with_capacity(dark_sums.len());
        let mut gains = Vec::with_capacity(dark_sums.len());
        for (dark_sum, bright_sum) in dark_sums.into_iter().zip(bright_sums) {
            // Subtract exact integer cross-products before the one division,
            // avoiding cancellation between separately rounded fractional means.
            let response = (f64::from(bright_sum) * dark.len() as f64
                - f64::from(dark_sum) * bright.len() as f64)
                / (dark.len() * bright.len()) as f64;
            ensure!(
                response >= config.minimum_response && response <= config.maximum_response,
                "flat-field pixel response outside configured bounds"
            );
            offsets.push(f64::from(dark_sum) / dark.len() as f64);
            gains.push(255.0 / response);
        }
        let mut profile = Self {
            width,
            height,
            config,
            dark_sources,
            bright_sources,
            offsets,
            gains,
            sha256: [0; 32],
        };
        profile.sha256 = profile.calculate_hash();
        profile.validate()?;
        Ok(profile)
    }

    fn validate_geometry(width: usize, height: usize) -> Result<()> {
        ensure!(
            width > 0
                && height > 0
                && width
                    .checked_mul(height)
                    .is_some_and(|n| n <= Self::MAX_PIXELS),
            "flat-field canvas exceeds 16 MiB or is empty"
        );
        Ok(())
    }
    fn calculate_hash(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"uareu.flat-field.profile.v1");
        for n in [self.width as u64, self.height as u64] {
            hash.update(n.to_le_bytes());
        }
        for n in [self.config.minimum_response, self.config.maximum_response] {
            hash.update(n.to_le_bytes());
        }
        for sources in [&self.dark_sources, &self.bright_sources] {
            hash.update((sources.len() as u64).to_le_bytes());
            for source in sources {
                hash.update((source.label.len() as u64).to_le_bytes());
                hash.update(source.label.as_bytes());
                hash.update(source.sha256);
            }
        }
        for values in [&self.offsets, &self.gains] {
            hash.update((values.len() as u64).to_le_bytes());
            for n in values {
                hash.update(n.to_le_bytes());
            }
        }
        hash.finalize().into()
    }
    pub fn sha256(&self) -> [u8; 32] {
        self.sha256
    }

    pub fn validate(&self) -> Result<()> {
        Self::validate_geometry(self.width, self.height)?;
        self.config.validate()?;
        for sources in [&self.dark_sources, &self.bright_sources] {
            ensure!(
                !sources.is_empty() && sources.len() <= Self::MAX_REFERENCES,
                "invalid flat-field reference count"
            );
            ensure!(
                sources
                    .iter()
                    .all(|s| !s.label.trim().is_empty() && s.label.len() <= 256),
                "invalid flat-field reference label"
            );
        }
        ensure!(
            self.offsets.len() == self.width * self.height
                && self.gains.len() == self.offsets.len(),
            "flat-field coefficient count mismatch"
        );
        for (&offset, &gain) in self.offsets.iter().zip(&self.gains) {
            ensure!(
                offset.is_finite()
                    && (0.0..=255.0).contains(&offset)
                    && gain.is_finite()
                    && gain > 0.0,
                "invalid flat-field coefficients"
            );
            let response = 255.0 / gain;
            ensure!(
                response.is_finite()
                    && response + 1e-10 >= self.config.minimum_response
                    && response <= self.config.maximum_response + 1e-10
                    && offset + response <= 255.0 + 1e-10,
                "invalid flat-field pixel response"
            );
        }
        ensure!(
            self.calculate_hash() == self.sha256,
            "flat-field profile hash mismatch"
        );
        Ok(())
    }
    /// Correct the complete canvas: round(clamp((pixel - dark_mean) * gain, 0, 255)).
    /// Preserves scale, valid region and existing processing history; never mutates
    /// the input. This is not vendor/PIV processing, dewarping or liveness detection.
    pub fn apply(&self, captured: &CapturedImage) -> Result<CapturedImage> {
        self.validate()?;
        Self::validate_geometry(captured.image.width, captured.image.height)?;
        captured.metadata.validate(&captured.image)?;
        ensure!(
            captured.image.width == self.width && captured.image.height == self.height,
            "flat-field input dimensions differ from profile"
        );
        let mut output = captured.clone();
        for ((pixel, &offset), &gain) in output
            .image
            .pixels
            .iter_mut()
            .zip(&self.offsets)
            .zip(&self.gains)
        {
            *pixel =
                (((f64::from(*pixel) - offset) * gain).clamp(0.0, 255.0) + 1e-10).round() as u8;
        }
        output
            .metadata
            .processing
            .push(ProcessingStep::FlatFieldCorrection {
                profile_sha256: self.sha256,
            });
        Ok(output)
    }
}
