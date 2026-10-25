// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit display processing; never substitutes for vendor correction/PIV.
use crate::{
    metadata::{CapturedImage, ProcessingStep, Region},
    quality::{analysis_region, diagnostics},
};
use anyhow::{Result, ensure};
use serde::Serialize;

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
}
