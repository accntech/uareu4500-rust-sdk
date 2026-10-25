// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional local SourceAFIS helper. Scores are not DigitalPersona scores.
use crate::{
    metadata::{CapturedImage, Region, Resolution},
    quality::analysis_region,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub mod enrollment;
pub use enrollment::{EnrollmentConfig, EnrollmentProgress, EnrollmentSession};

#[derive(Clone, Debug)]
pub struct RecognitionEngine {
    pub executable: PathBuf,
    pub timeout: Duration,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecognitionIdentity {
    pub engine: String,
    pub version: String,
    pub engine_sha256: String,
    pub host_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FingerprintTemplate {
    pub schema_version: u32,
    pub identity: RecognitionIdentity,
    pub format: String,
    pub data: Vec<u8>,
    pub source_resolution: Resolution,
    pub source_region: Region,
    pub source_pixels_sha256: Option<String>,
    #[serde(default)]
    pub source_processing: Vec<crate::metadata::ProcessingStep>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Enrollment {
    pub schema_version: u32,
    pub policy: String,
    pub samples: Vec<FingerprintTemplate>,
    pub consistency_threshold: f64,
    pub minimum_pair_score: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MatchResult {
    pub score: f64,
    pub threshold: f64,
    pub matched: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub index: usize,
    pub score: f64,
}

const REQUEST_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
struct ComparisonBatch<'a> {
    operation: &'static str,
    probe: &'a [u8],
    candidate: &'a [u8],
    additional_candidates: Vec<&'a [u8]>,
}

struct RequestCounter {
    length: usize,
    overflow: bool,
}
impl Write for RequestCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > REQUEST_LIMIT.saturating_sub(self.length) {
            self.overflow = true;
            return Err(std::io::Error::other("recognition request exceeds 16 MiB"));
        }
        self.length += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn request_fits(request: &impl Serialize) -> Result<bool> {
    let mut counter = RequestCounter {
        length: 0,
        overflow: false,
    };
    let result = serde_json::to_writer(&mut counter, request);
    if counter.overflow {
        return Ok(false);
    }
    result?;
    Ok(true)
}
fn score(value: &serde_json::Value) -> Result<f64> {
    let score = value.as_f64().context("missing similarity score")?;
    ensure!(
        score.is_finite() && score >= 0.0,
        "invalid similarity score"
    );
    Ok(score)
}

impl RecognitionEngine {
    fn execute(&self, request: &impl Serialize) -> Result<serde_json::Value> {
        let start = Instant::now();
        ensure!(
            self.timeout > Duration::ZERO && self.timeout <= Duration::from_secs(300),
            "invalid engine deadline"
        );
        ensure!(
            self.executable.is_file(),
            "recognition helper unavailable; build examples/engine and configure its executable"
        );
        let temp = Workspace::new()?;
        let input = serde_json::to_vec(request)?;
        ensure!(
            input.len() <= REQUEST_LIMIT,
            "recognition request exceeds 16 MiB"
        );
        let input_path = temp.0.join("request.json");
        private_file(&input_path)?.write_all(&input)?;
        let stdout = temp.0.join("stdout.json");
        let stderr = temp.0.join("stderr.txt");
        let mut command = Command::new(fs::canonicalize(&self.executable)?);
        command
            .stdin(File::open(input_path)?)
            .stdout(private_file(&stdout)?)
            .stderr(private_file(&stderr)?);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Child(command.spawn().context("cannot start recognition helper")?);
        loop {
            ensure!(
                fs::metadata(&stdout)?.len() <= 16 * 1024 * 1024
                    && fs::metadata(&stderr)?.len() <= 65536,
                "recognition output limit exceeded"
            );
            ensure!(start.elapsed() < self.timeout, "recognition timed out");
            if let Some(status) = child.0.try_wait()? {
                if !status.success() {
                    let message = read_bounded(&stderr, 65536)?;
                    bail!(
                        "recognition failed: {}",
                        String::from_utf8_lossy(&message).trim()
                    );
                }
                let mut bytes = Vec::new();
                File::open(stdout)?
                    .take(16 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() <= 16 * 1024 * 1024,
                    "recognition output limit exceeded"
                );
                let response: serde_json::Value = serde_json::from_slice(&bytes)?;
                ensure!(
                    response["schema_version"] == 1
                        && response["engine"] == "SourceAFIS"
                        && response["version"] == "3.14.0",
                    "unsupported recognition protocol/engine version"
                );
                return Ok(response);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn extract(&self, captured: &CapturedImage) -> Result<FingerprintTemplate> {
        captured.metadata.validate(&captured.image)?;
        let Resolution::Known { x_ppi, y_ppi, .. } = captured.metadata.resolution else {
            bail!(
                "template extraction requires explicit resolution; unknown resolution is not guessed"
            );
        };
        ensure!(
            (x_ppi - y_ppi).abs() < 0.0128 && (250.0..=2000.0).contains(&x_ppi),
            "unsupported recognition pixel scale"
        );
        let region = analysis_region(captured);
        ensure!(
            region.width >= 16
                && region.height >= 16
                && region.width <= 2048
                && region.height <= 2048
                && region.width * region.height <= 1024 * 1024,
            "unsupported recognition image size"
        );
        let mut pixels = Vec::with_capacity(region.width * region.height);
        for y in region.y..region.y + region.height {
            pixels.extend_from_slice(
                &captured.image.pixels[y * captured.image.width + region.x
                    ..y * captured.image.width + region.x + region.width],
            );
        }
        let response=self.execute(&serde_json::json!({"operation":"extract","width":region.width,"height":region.height,"ppi":x_ppi,"pixels":pixels}))?;
        let data: Vec<u8> = serde_json::from_value(response["result"]["data"].clone())?;
        ensure!(
            !data.is_empty() && data.len() <= 1024 * 1024,
            "invalid template payload size"
        );
        Ok(FingerprintTemplate {
            schema_version: 1,
            identity: serde_json::from_value(response.clone())?,
            format: "sourceafis_native_cache".into(),
            data,
            source_resolution: captured.metadata.resolution.clone(),
            source_region: region,
            source_pixels_sha256: Some(format!("{:x}", Sha256::digest(&pixels))),
            source_processing: captured.metadata.processing.clone(),
        })
    }
    fn validate(&self, template: &FingerprintTemplate) -> Result<()> {
        ensure!(
            template.schema_version == 1
                && template.format == "sourceafis_native_cache"
                && template.identity.engine == "SourceAFIS"
                && template.identity.version == "3.14.0",
            "unsupported template engine/format/version"
        );
        ensure!(
            !template.data.is_empty() && template.data.len() <= 1024 * 1024,
            "invalid template size"
        );
        template.source_resolution.validate()?;
        ensure!(
            matches!(template.source_resolution, Resolution::Known { .. }),
            "template provenance lacks resolution"
        );
        Ok(())
    }
    pub fn compare(
        &self,
        probe: &FingerprintTemplate,
        candidate: &FingerprintTemplate,
        threshold: f64,
    ) -> Result<MatchResult> {
        self.validate(probe)?;
        self.validate(candidate)?;
        ensure!(
            threshold.is_finite() && threshold > 0.0,
            "explicit positive finite threshold required"
        );
        ensure!(
            probe.identity.engine_sha256 == candidate.identity.engine_sha256,
            "templates use different engine builds"
        );
        let response=self.execute(&serde_json::json!({"operation":"compare","probe":probe.data,"candidate":candidate.data}))?;
        ensure!(
            response["engine_sha256"] == probe.identity.engine_sha256,
            "engine build differs from template; re-extract from original images"
        );
        let score = score(&response["result"]["score"])?;
        Ok(MatchResult {
            score,
            threshold,
            matched: score >= threshold,
        })
    }
    pub fn identify(
        &self,
        probe: &FingerprintTemplate,
        candidates: &[FingerprintTemplate],
        threshold: f64,
        maximum: usize,
    ) -> Result<Vec<Candidate>> {
        self.validate(probe)?;
        ensure!(
            threshold.is_finite() && threshold > 0.0,
            "explicit positive finite threshold required"
        );
        ensure!(
            candidates.len() <= 1000 && maximum > 0 && maximum <= 1000,
            "invalid candidate count"
        );
        let scores = self.comparison_scores(probe, candidates, threshold)?;
        let mut results: Vec<_> = scores
            .into_iter()
            .enumerate()
            .filter(|(_, score)| *score >= threshold)
            .map(|(index, score)| Candidate { index, score })
            .collect();
        results.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.index.cmp(&b.index)));
        results.truncate(maximum);
        Ok(results)
    }
    fn comparison_scores(
        &self,
        probe: &FingerprintTemplate,
        candidates: &[FingerprintTemplate],
        threshold: f64,
    ) -> Result<Vec<f64>> {
        let started = Instant::now();
        self.validate(probe)?;
        for candidate in candidates {
            self.validate(candidate)?;
            ensure!(
                probe.identity.engine_sha256 == candidate.identity.engine_sha256,
                "templates use different engine builds"
            );
        }
        let Some((first, additional)) = candidates.split_first() else {
            return Ok(Vec::new());
        };
        ensure!(
            self.timeout > Duration::ZERO && self.timeout <= Duration::from_secs(300),
            "invalid engine deadline"
        );
        let remaining_engine = || -> Result<Self> {
            let remaining = self.timeout.saturating_sub(started.elapsed());
            ensure!(!remaining.is_zero(), "recognition timed out");
            Ok(Self {
                executable: self.executable.clone(),
                timeout: remaining,
            })
        };
        let batch = ComparisonBatch {
            operation: "compare",
            probe: &probe.data,
            candidate: &first.data,
            additional_candidates: additional.iter().map(|t| t.data.as_slice()).collect(),
        };
        let mut scores = Vec::with_capacity(candidates.len());
        let legacy = if request_fits(&batch)? {
            let response = remaining_engine()?.execute(&batch)?;
            ensure!(
                response["engine_sha256"] == probe.identity.engine_sha256,
                "engine build differs from template; re-extract from original images"
            );
            scores.push(score(&response["result"]["score"])?);
            if let Some(extra) = response["result"].get("additional_scores") {
                let extra = extra
                    .as_array()
                    .context("invalid additional similarity scores")?;
                ensure!(
                    extra.len() == additional.len(),
                    "invalid additional similarity score count"
                );
                scores.extend(extra.iter().map(score).collect::<Result<Vec<_>>>()?);
                return Ok(scores);
            }
            // Old helpers ignore additive request fields. Their first result is
            // already valid, so compare only the remainder under the same budget.
            additional
        } else {
            // The pre-existing API caps each comparison request, not the gallery
            // aggregate. Keep large previously valid galleries on that path.
            candidates
        };
        for candidate in legacy {
            scores.push(
                remaining_engine()?
                    .compare(probe, candidate, threshold)?
                    .score,
            );
        }
        Ok(scores)
    }
    /// Multi-template enrollment with explicit consistency policy, not vendor fusion.
    pub fn enroll(&self, samples: &[FingerprintTemplate], threshold: f64) -> Result<Enrollment> {
        ensure!(
            (2..=10).contains(&samples.len()),
            "enrollment requires 2..10 samples"
        );
        let started = Instant::now();
        let mut minimum = f64::INFINITY;
        for i in 0..samples.len() {
            for j in i + 1..samples.len() {
                let remaining = self.timeout.saturating_sub(started.elapsed());
                ensure!(!remaining.is_zero(), "enrollment timed out");
                let engine = Self {
                    executable: self.executable.clone(),
                    timeout: remaining,
                };
                let matched = engine.compare(&samples[i], &samples[j], threshold)?;
                ensure!(matched.matched, "inconsistent enrollment samples");
                minimum = minimum.min(matched.score);
            }
        }
        Ok(Enrollment {
            schema_version: 1,
            policy: "all_pairs_consistent_multi_template_not_vendor_fusion".into(),
            samples: samples.to_vec(),
            consistency_threshold: threshold,
            minimum_pair_score: minimum,
        })
    }
    pub fn verify_enrollment(
        &self,
        probe: &FingerprintTemplate,
        enrollment: &Enrollment,
        threshold: f64,
    ) -> Result<MatchResult> {
        ensure!(
            enrollment.schema_version == 1 && (2..=10).contains(&enrollment.samples.len()),
            "invalid enrollment"
        );
        ensure!(
            threshold.is_finite() && threshold > 0.0,
            "explicit positive finite threshold required"
        );
        let score = self
            .comparison_scores(probe, &enrollment.samples, threshold)?
            .into_iter()
            .fold(0.0f64, f64::max);
        Ok(MatchResult {
            score,
            threshold,
            matched: score >= threshold,
        })
    }
    /// Explicit single-view export. Quality/impression are caller declarations;
    /// no PIV certification, quality score, or liveness result is invented.
    pub fn export_template(
        &self,
        template: &FingerprintTemplate,
        format: crate::template_io::TemplateFormat,
        quality: u8,
        impression: u8,
    ) -> Result<Vec<u8>> {
        self.validate(template)?;
        let response =
            self.execute(&serde_json::json!({"operation":"minutiae","template":template.data}))?;
        ensure!(
            response["engine_sha256"] == template.identity.engine_sha256,
            "engine build differs from template"
        );
        #[derive(Deserialize)]
        struct Native {
            width: u16,
            height: u16,
            positions_x: Vec<u16>,
            positions_y: Vec<u16>,
            directions: Vec<f64>,
            types: String,
        }
        let native: Native = serde_json::from_value(response["result"].clone())?;
        let chars: Vec<_> = native.types.chars().collect();
        ensure!(
            chars.len() == native.positions_x.len()
                && chars.len() == native.positions_y.len()
                && chars.len() == native.directions.len(),
            "inconsistent native minutiae arrays"
        );
        let mut minutiae = Vec::new();
        for (index, kind) in chars.into_iter().enumerate() {
            minutiae.push(crate::template_io::Minutia {
                x: native.positions_x[index],
                y: native.positions_y[index],
                kind: match kind {
                    'E' => 1,
                    'B' => 2,
                    _ => anyhow::bail!("unsupported native minutia type"),
                },
                direction_clockwise: native.directions[index],
                quality: 0,
            });
        }
        crate::template_io::encode(
            &crate::template_io::MinutiaeRecord {
                width: native.width,
                height: native.height,
                pixels_per_cm_x: 197,
                pixels_per_cm_y: 197,
                finger_position: 0,
                impression,
                quality,
                vendor_id: 0x0103,
                subformat: 0,
                device_info: 0,
                minutiae,
            },
            format,
        )
    }
    pub fn import_template(
        &self,
        bytes: &[u8],
        format: crate::template_io::TemplateFormat,
    ) -> Result<FingerprintTemplate> {
        let mut record = crate::template_io::decode(bytes, format)?;
        ensure!(
            record.pixels_per_cm_x == record.pixels_per_cm_y,
            "SourceAFIS import requires square-pixel coordinates"
        );
        let ppi = record.pixels_per_cm_x as f64 * 2.54;
        let source_resolution = Resolution::Known {
            x_ppi: ppi,
            y_ppi: ppi,
            provenance: crate::metadata::ResolutionProvenance::TemplateHeaderDeclaration {
                format: format!("{format:?}"),
            },
        };
        let source_region = Region {
            x: 0,
            y: 0,
            width: record.width as usize,
            height: record.height as usize,
        };
        let scale = 500.0 / ppi;
        ensure!(
            record.width as f64 * scale <= 10000.0 && record.height as f64 * scale <= 10000.0,
            "normalized template geometry too large"
        );
        record.width = (record.width as f64 * scale).round().max(1.0) as u16;
        record.height = (record.height as f64 * scale).round().max(1.0) as u16;
        for m in &mut record.minutiae {
            ensure!(
                m.kind == 1 || m.kind == 2,
                "SourceAFIS cannot represent other minutia types"
            );
            m.x = (m.x as f64 * scale).round() as u16;
            m.y = (m.y as f64 * scale).round() as u16;
        }
        let response =
            self.execute(&serde_json::json!({"operation":"import_minutiae","record":record}))?;
        let data: Vec<u8> = serde_json::from_value(response["result"]["data"].clone())?;
        Ok(FingerprintTemplate {
            schema_version: 1,
            identity: serde_json::from_value(response.clone())?,
            format: "sourceafis_native_cache".into(),
            data,
            source_resolution,
            source_region,
            source_pixels_sha256: None,
            source_processing: Vec::new(),
        })
    }
}

pub(crate) fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= maximum, "engine output limit exceeded");
    Ok(bytes)
}
pub(crate) struct Workspace(pub(crate) PathBuf);
impl Workspace {
    pub(crate) fn new() -> Result<Self> {
        let root = fs::canonicalize(std::env::temp_dir())?;
        for _ in 0..10 {
            let mut nonce = [0; 16];
            getrandom::fill(&mut nonce)?;
            let path = root.join(format!("uareu-engine-{:032x}", u128::from_le_bytes(nonce)));
            #[allow(unused_mut)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        bail!("cannot allocate recognition workspace")
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if self.0.is_absolute()
            && self
                .0
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("uareu-engine-"))
            && fs::canonicalize(std::env::temp_dir()).ok().as_deref() == self.0.parent()
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
pub(crate) fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_resolution_is_rejected_before_any_engine_invocation() {
        let engine = RecognitionEngine {
            executable: "absent".into(),
            timeout: Duration::from_secs(1),
        };
        let image = CapturedImage::new(crate::frame::Image {
            width: 32,
            height: 32,
            pixels: vec![255; 1024],
        });
        assert!(
            engine
                .extract(&image)
                .unwrap_err()
                .to_string()
                .contains("resolution")
        );
    }
}
