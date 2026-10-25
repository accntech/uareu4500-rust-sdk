// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional NBIS 5.0.0 legacy NFIQ 1 CLI integration. Never NFIQ 2 scores.
use super::{
    EngineIdentity, analysis_region,
    nfiq2::{Workspace, canonical_cli_path, hash_file, private_file, run},
};
use crate::metadata::{CapturedImage, Region, Resolution};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    ffi::OsStr,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Nfiq1 {
    pub executable: PathBuf,
    /// One deadline for version validation and scoring, capped at five minutes.
    pub timeout: Duration,
}

/// Separate legacy result: 1 is best, 5 is worst. No conversion to NFIQ 2.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LegacyNfiqOutcome {
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
        engine: Box<EngineIdentity>,
        /// Legacy NFIQ class in 1..=5, lower is better.
        score: u8,
        score_direction: &'static str,
        input_region: Region,
        input_resolution: Resolution,
        evaluation_ppi: u32,
    },
}

impl Nfiq1 {
    /// Evaluate explicitly declared 500-PPI, 8-bit single-finger input.
    /// The adapter accepts a 256..1024-pixel rectangle per axis and never resamples.
    pub fn assess(&self, captured: &CapturedImage) -> LegacyNfiqOutcome {
        let unsupported = |reason: String| LegacyNfiqOutcome::UnsupportedInput { reason };
        if let Err(e) = captured.metadata.validate(&captured.image) {
            return unsupported(format!("{e:#}"));
        }
        let Resolution::Known { x_ppi, y_ppi, .. } = captured.metadata.resolution else {
            return unsupported("NFIQ 1 requires an explicit 500-PPI declaration".into());
        };
        if (x_ppi - 500.0).abs() > 0.0128 || (y_ppi - 500.0).abs() > 0.0128 {
            return unsupported(
                "NFIQ 1 requires 500 PPI on both axes; resampling is not implemented".into(),
            );
        }
        let region = analysis_region(captured);
        if !(256..=1024).contains(&region.width) || !(256..=1024).contains(&region.height) {
            return unsupported("NFIQ 1 adapter accepts 256..1024 pixels on each axis".into());
        }
        if !self.executable.is_file() {
            return LegacyNfiqOutcome::EngineUnavailable {
                reason: "NBIS NFIQ 1 executable was not found".into(),
            };
        }
        match self.evaluate(captured, region) {
            Ok(result) => result,
            Err(e) => LegacyNfiqOutcome::EvaluationFailed {
                reason: format!("{e:#}"),
            },
        }
    }

    /// Select one raw ISO record view. Opaque compressed content is not decoded.
    pub fn assess_record_view(
        &self,
        record: &crate::image_record::MultiImageRecord,
        view: usize,
    ) -> LegacyNfiqOutcome {
        let mut captured = match record.raw_image(view) {
            Ok(image) => image,
            Err(e) => {
                return LegacyNfiqOutcome::UnsupportedInput {
                    reason: format!("{e:#}"),
                };
            }
        };
        let declared = captured.metadata.resolution.clone();
        // Carrier quantization only; retain the original declaration in the result.
        if record.scale_units == 2
            && record.image_resolution_x == 197
            && record.image_resolution_y == 197
        {
            if let Resolution::Known { provenance, .. } = captured.metadata.resolution.clone() {
                captured.metadata.resolution = Resolution::Known {
                    x_ppi: 500.0,
                    y_ppi: 500.0,
                    provenance,
                };
            }
        }
        let mut result = self.assess(&captured);
        if let LegacyNfiqOutcome::Scored {
            input_resolution, ..
        } = &mut result
        {
            *input_resolution = declared;
        }
        result
    }

    fn evaluate(&self, captured: &CapturedImage, region: Region) -> Result<LegacyNfiqOutcome> {
        ensure!(
            !self.timeout.is_zero() && self.timeout <= Duration::from_secs(300),
            "quality timeout must be greater than zero and at most 300 seconds"
        );
        let started = Instant::now();
        let remaining = || -> Result<Duration> {
            let remaining = self.timeout.saturating_sub(started.elapsed());
            ensure!(!remaining.is_zero(), "legacy NFIQ evaluation timed out");
            Ok(remaining)
        };
        let executable = canonical_cli_path(&self.executable)
            .context("cannot resolve NBIS NFIQ 1 executable")?;
        let executable_sha256 = hash_file(&executable)?;
        let workspace = Workspace::new()?;
        let version = run(
            &executable,
            &[OsStr::new("-version")],
            &workspace.0,
            "legacy-version",
            remaining()?,
        )?;
        let version = std::str::from_utf8(&version).context("NBIS version is not UTF-8")?;
        ensure!(
            version.lines().map(str::trim).eq([
                "Standard Version: ANSI/NIST-ITL 1-2007",
                "NBIS Non-Export Control Software Version: Release 5.0.0"
            ]),
            "unsupported NBIS NFIQ CLI version; expected Release 5.0.0"
        );
        let path = workspace.0.join("input.raw");
        let mut raw = private_file(&path)?;
        for y in region.y..region.y + region.height {
            let offset = y * captured.image.width + region.x;
            raw.write_all(&captured.image.pixels[offset..offset + region.width])?;
        }
        drop(raw);
        let geometry = format!("{},{},8,500", region.width, region.height);
        let output = run(
            &executable,
            &[
                OsStr::new("-o"),
                OsStr::new("-raw"),
                OsStr::new(&geometry),
                path.as_os_str(),
            ],
            &workspace.0,
            "legacy-score",
            remaining()?,
        )?;
        let output = std::str::from_utf8(&output)
            .context("NFIQ 1 score is not UTF-8")?
            .trim();
        ensure!(
            matches!(output, "1" | "2" | "3" | "4" | "5"),
            "expected exactly one legacy NFIQ class in 1..5"
        );
        Ok(LegacyNfiqOutcome::Scored {
            engine: Box::new(EngineIdentity {
                name: "NFIQ 1",
                version: "NBIS 5.0.0".into(),
                executable_sha256,
                model_source: "embedded NBIS nfiqgbls weights",
                model_info_sha256: None,
                model_data_sha256: None,
            }),
            score: output.parse()?,
            score_direction: "lower_is_better",
            input_region: region,
            input_resolution: captured.metadata.resolution.clone(),
            evaluation_ppi: 500,
        })
    }
}
