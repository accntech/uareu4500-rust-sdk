// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional NBIS cwsq/dwsq adapter. No codec is bundled or certification claimed.
use crate::{
    frame::Image,
    metadata::{CapturedImage, ProcessingStep, Resolution, ResolutionProvenance},
    process::{Child, Workspace, private_file, read_bounded},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct WsqCodec {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub timeout: Duration,
}
/// Bounds for a measured bitrate search. The codec's per-process timeout also applies.
#[derive(Clone, Copy, Debug)]
pub struct WsqTargetOptions {
    pub target_bytes: usize,
    /// Total encoder invocations, including failed/timed-out invocations (1..=32).
    pub max_encodes: usize,
    /// One shared wall-clock budget for all attempts (0..=300 seconds, exclusive of zero).
    pub deadline: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsqTargetStatus {
    /// At least one measured candidate fits. Search may stop at either budget.
    Met,
    /// The minimum supported bitrate exceeded the target. No global size guarantee
    /// is inferred: encoded size need not be strictly monotonic with bitrate.
    Unattainable,
    EncodeLimit,
    Deadline,
    CodecFailure(String),
}

#[derive(Clone, Debug)]
pub struct WsqTargetResult {
    /// Highest tested fitting bitrate, or smallest valid output if none fitted.
    pub bytes: Vec<u8>,
    pub bitrate: f64,
    pub encodes: usize,
    pub status: WsqTargetStatus,
}

impl WsqCodec {
    /// Search 0.1..=2.25 bits/pixel using measured output lengths. This is a
    /// maximum byte target, not an exact-size or image-quality promise. Search
    /// tests both endpoints then bisects. A valid candidate survives later
    /// failures/deadlines. If no attempt succeeds, return the underlying error.
    pub fn encode_to_target(
        &self,
        captured: &CapturedImage,
        options: WsqTargetOptions,
    ) -> Result<WsqTargetResult> {
        ensure!(
            options.target_bytes > 0 && options.target_bytes <= 4 * 1024 * 1024,
            "WSQ target size must be 1..4194304 bytes"
        );
        ensure!(
            (1..=32).contains(&options.max_encodes),
            "WSQ max_encodes must be 1..32"
        );
        ensure!(
            options.deadline > Duration::ZERO && options.deadline <= Duration::from_secs(300),
            "WSQ search deadline out of range"
        );
        ensure!(
            self.timeout > Duration::ZERO && self.timeout <= Duration::from_secs(300),
            "codec deadline out of range"
        );
        let start = Instant::now();
        let mut best: Option<WsqTargetResult> = None;
        let (mut low, mut high) = (0.1, 2.25);
        let mut rate = high;
        for attempt in 1..=options.max_encodes {
            let remaining = options.deadline.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                if let Some(mut result) = best {
                    result.status = if result.bytes.len() <= options.target_bytes {
                        WsqTargetStatus::Met
                    } else {
                        WsqTargetStatus::Deadline
                    };
                    return Ok(result);
                }
                anyhow::bail!("NBIS search deadline expired before a valid output");
            }
            let bounded = Self {
                timeout: self.timeout.min(remaining),
                ..self.clone()
            };
            let bytes = match bounded.encode(captured, rate) {
                Ok(bytes) => bytes,
                Err(error) => {
                    if let Some(mut result) = best {
                        result.encodes = attempt;
                        result.status = if start.elapsed() >= options.deadline {
                            WsqTargetStatus::Deadline
                        } else {
                            WsqTargetStatus::CodecFailure(error.to_string())
                        };
                        return Ok(result);
                    }
                    return Err(error);
                }
            };
            let fits = bytes.len() <= options.target_bytes;
            let replace = best.as_ref().is_none_or(|old| {
                let old_fits = old.bytes.len() <= options.target_bytes;
                if fits {
                    !old_fits || rate > old.bitrate
                } else {
                    !old_fits && bytes.len() < old.bytes.len()
                }
            });
            if replace {
                best = Some(WsqTargetResult {
                    bytes,
                    bitrate: rate,
                    encodes: attempt,
                    status: WsqTargetStatus::EncodeLimit,
                });
            }
            let result = best
                .as_mut()
                .expect("successful encode establishes candidate");
            result.encodes = attempt;
            if attempt == 1 && fits {
                result.status = WsqTargetStatus::Met;
                return Ok(best.expect("candidate exists"));
            }
            if attempt == 2 && !fits {
                result.status = WsqTargetStatus::Unattainable;
                return Ok(best.expect("candidate exists"));
            }
            if fits {
                low = rate;
            } else {
                high = rate;
            }
            rate = if attempt == 1 {
                low
            } else {
                (low + high) / 2.0
            };
        }
        let mut result = best.context("NBIS search produced no output")?;
        if result.bytes.len() <= options.target_bytes {
            result.status = WsqTargetStatus::Met;
        }
        Ok(result)
    }

    fn run(&self, executable: &Path, args: &[OsString], workspace: &Path) -> Result<()> {
        ensure!(executable.is_file(), "NBIS codec executable unavailable");
        ensure!(
            self.timeout > Duration::ZERO && self.timeout <= Duration::from_secs(300),
            "codec deadline out of range"
        );
        let stdout = workspace.join("stdout.txt");
        let stderr = workspace.join("stderr.txt");
        let mut command = Command::new(fs::canonicalize(executable)?);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(private_file(&stdout)?)
            .stderr(private_file(&stderr)?);
        let child = Child::spawn(&mut command)?;
        let start = Instant::now();
        let status = child.wait_checked(|| {
            ensure!(start.elapsed() < self.timeout, "NBIS codec timed out");
            for path in fs::read_dir(workspace)? {
                ensure!(
                    path?.metadata()?.len() <= 4 * 1024 * 1024,
                    "codec output limit exceeded"
                );
            }
            Ok(())
        })?;
        ensure!(
            status.success(),
            "NBIS codec failed: {}",
            String::from_utf8_lossy(&read_bounded(&stderr, 65536)?)
        );
        Ok(())
    }
    pub fn encode(&self, captured: &CapturedImage, bitrate: f64) -> Result<Vec<u8>> {
        captured.metadata.validate(&captured.image)?;
        ensure!(
            bitrate.is_finite() && (0.1..=2.25).contains(&bitrate),
            "WSQ bitrate must be 0.1..2.25 bits/pixel"
        );
        let Resolution::Known { x_ppi, y_ppi, .. } = captured.metadata.resolution else {
            anyhow::bail!("WSQ encoding requires explicit resolution");
        };
        ensure!(
            (x_ppi - 500.0).abs() < 0.0128 && (y_ppi - 500.0).abs() < 0.0128,
            "this WSQ adapter accepts only explicitly declared 500-PPI images"
        );
        let image = &captured.image;
        ensure!(
            image.width >= 256
                && image.height >= 256
                && image.width <= 2048
                && image.height <= 2048
                && image.pixels.len() <= 1024 * 1024,
            "unsupported WSQ image size"
        );
        let workspace = Workspace::new()?;
        let raw = workspace.0.join("image.raw");
        private_file(&raw)?.write_all(&image.pixels)?;
        self.run(
            &self.encoder,
            &[
                bitrate.to_string().into(),
                "wsq".into(),
                raw.into_os_string(),
                "-raw_in".into(),
                format!("{},{},8,500", image.width, image.height).into(),
            ],
            &workspace.0,
        )?;
        let bytes = read_bounded(&workspace.0.join("image.wsq"), 4 * 1024 * 1024)?;
        ensure!(
            bytes.starts_with(&[0xff, 0xa0])
                && bytes.ends_with(&[0xff, 0xa1])
                && bytes.len() <= 4 * 1024 * 1024,
            "invalid WSQ codec output"
        );
        Ok(bytes)
    }
    pub fn decode(&self, bytes: &[u8]) -> Result<CapturedImage> {
        ensure!(
            bytes.len() <= 4 * 1024 * 1024
                && bytes.starts_with(&[0xff, 0xa0])
                && bytes.ends_with(&[0xff, 0xa1]),
            "invalid WSQ envelope"
        );
        let workspace = Workspace::new()?;
        let input = workspace.0.join("image.wsq");
        private_file(&input)?.write_all(bytes)?;
        self.run(
            &self.decoder,
            &["raw".into(), input.into_os_string(), "-raw_out".into()],
            &workspace.0,
        )?;
        let metadata = String::from_utf8(read_bounded(&workspace.0.join("image.ncm"), 16384)?)
            .context("NBIS decoder did not emit NISTCOM metadata")?;
        ensure!(metadata.len() <= 16384, "NISTCOM metadata too large");
        let mut fields = BTreeMap::new();
        for line in metadata.lines() {
            if let Some((name, value)) = line.split_once(char::is_whitespace) {
                ensure!(
                    fields.insert(name, value.trim()).is_none(),
                    "duplicate NISTCOM key"
                );
            }
        }
        let number = |name| -> Result<usize> {
            Ok(fields
                .get(name)
                .with_context(|| format!("missing {name}"))?
                .parse()?)
        };
        let (width, height) = (number("PIX_WIDTH")?, number("PIX_HEIGHT")?);
        ensure!(
            width > 0
                && height > 0
                && width <= 2048
                && height <= 2048
                && width * height <= 1024 * 1024
                && number("PIX_DEPTH")? == 8,
            "unsupported WSQ decoded geometry"
        );
        let pixels = read_bounded(&workspace.0.join("image.raw"), 1024 * 1024)?;
        ensure!(
            pixels.len() == width * height,
            "decoded WSQ length mismatch"
        );
        let mut image = CapturedImage::new(Image {
            width,
            height,
            pixels,
        });
        if let Some(ppi) = fields
            .get("PPI")
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|ppi| *ppi > 0.0)
        {
            let resolution = Resolution::Known {
                x_ppi: ppi,
                y_ppi: ppi,
                provenance: ResolutionProvenance::WsqCommentDeclaration,
            };
            resolution.validate()?;
            image.metadata.resolution = resolution;
        }
        image
            .metadata
            .processing
            .push(ProcessingStep::WsqDecompression);
        Ok(image)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scale_and_envelope_checks_precede_external_tools() {
        let codec = WsqCodec {
            encoder: "absent".into(),
            decoder: "absent".into(),
            timeout: Duration::from_secs(1),
        };
        let image = CapturedImage::new(Image {
            width: 256,
            height: 256,
            pixels: vec![128; 65536],
        });
        assert!(
            codec
                .encode(&image, 0.75)
                .unwrap_err()
                .to_string()
                .contains("resolution")
        );
        assert!(
            codec
                .decode(&[0, 1, 2])
                .unwrap_err()
                .to_string()
                .contains("envelope")
        );
    }
}
