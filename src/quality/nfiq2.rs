// SPDX-License-Identifier: GPL-3.0-or-later
use super::{EngineIdentity, QualityOutcome, analysis_region};
use crate::{
    frame::Image,
    metadata::{CapturedImage, Resolution},
};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: u64 = 128 * 1024;
const SUPPORTED_VERSION: &str = "2.3.0";

/// Optional, externally installed NIST CLI. No executable is downloaded or bundled.
/// Explicit model information selects and fingerprints the model used by the CLI.
#[derive(Clone, Debug)]
pub struct Nfiq2 {
    pub executable: PathBuf,
    pub model_info: Option<PathBuf>,
    /// Per-process deadline, including the version probe. Maximum five minutes.
    pub timeout: Duration,
}

impl Nfiq2 {
    /// Score one explicitly selected raw ISO image-record view. The header's
    /// resolution is retained as a declaration, not a hardware measurement.
    /// Compressed records must be independently decoded and verified first.
    pub fn assess_record_view(
        &self,
        record: &crate::image_record::MultiImageRecord,
        view: usize,
    ) -> QualityOutcome {
        match record.raw_image(view) {
            Ok(mut image) => {
                // ISO integer pixels/cm rounds nominal 500 PPI to 197 PPCM.
                // Normalize only that documented carrier quantization for the
                // scorer; do not mutate the record or infer measured reader DPI.
                if record.scale_units == 2
                    && record.image_resolution_x == 197
                    && record.image_resolution_y == 197
                {
                    if let Resolution::Known { provenance, .. } = image.metadata.resolution.clone()
                    {
                        image.metadata.resolution = Resolution::Known {
                            x_ppi: 500.0,
                            y_ppi: 500.0,
                            provenance,
                        };
                    }
                }
                self.assess(&image)
            }
            Err(error) => QualityOutcome::UnsupportedInput {
                reason: format!("{error:#}"),
            },
        }
    }

    pub fn assess(&self, captured: &CapturedImage) -> QualityOutcome {
        let unsupported = |reason: &str| QualityOutcome::UnsupportedInput {
            reason: reason.into(),
        };
        if let Err(error) = captured.metadata.validate(&captured.image) {
            return unsupported(&format!("{error:#}"));
        }
        let Resolution::Known { x_ppi, y_ppi, .. } = captured.metadata.resolution else {
            return unsupported(
                "resolution is unknown; supply an explicit declaration, not an assumed default",
            );
        };
        // Half a PNG pixels/metre unit is 0.0127 PPI. Accept its round-trip error.
        if (x_ppi - 500.0).abs() > 0.0128 || (y_ppi - 500.0).abs() > 0.0128 {
            return unsupported(
                "NFIQ 2 requires 500 PPI on both axes; resampling is not implemented",
            );
        }
        let region = analysis_region(captured);
        if region.width <= 196
            || region.width >= 800
            || region.height <= 196
            || region.height >= 1000
        {
            return unsupported(
                "NFIQ 2 requires width 197..799 and height 197..999; engine whitespace cropping may impose further limits",
            );
        }
        if !self.executable.is_file() {
            return QualityOutcome::EngineUnavailable {
                reason: "NFIQ 2 executable was not found; supply its file path".into(),
            };
        }
        match self.evaluate(captured) {
            Ok(outcome) => outcome,
            Err(error) => QualityOutcome::EvaluationFailed {
                reason: format!("{error:#}"),
            },
        }
    }

    fn evaluate(&self, captured: &CapturedImage) -> Result<QualityOutcome> {
        ensure!(
            !self.timeout.is_zero() && self.timeout <= Duration::from_secs(300),
            "engine timeout must be greater than zero and at most 300 seconds"
        );
        let executable =
            canonical_cli_path(&self.executable).context("cannot resolve NFIQ 2 executable")?;
        let temporary = Workspace::new()?;
        // The pinned CLI prints successful usage/version output with no arguments.
        // It does not implement a -h switch.
        let version = run(&executable, &[], &temporary.0, "version", self.timeout)?;
        let help = String::from_utf8(version).context("NFIQ 2 help is not UTF-8")?;
        let reported = help
            .lines()
            .find_map(|line| line.trim().strip_prefix("NFIQ 2:"))
            .and_then(|value| value.split_whitespace().next());
        ensure!(
            reported == Some(SUPPORTED_VERSION),
            "unsupported or unrecognized NFIQ 2 version; expected 2.3.0"
        );
        let embedded = help
            .lines()
            .any(|line| line.trim() == "Embedded model present");
        let model_info = self
            .model_info
            .as_ref()
            .map(|path| canonical_cli_path(path))
            .transpose()
            .context("cannot resolve NFIQ 2 model information")?;
        let (model_info_sha256, model_data_sha256) = if embedded {
            ensure!(
                model_info.is_none(),
                "embedded-model NFIQ 2 builds ignore -m; omit the model option"
            );
            (None, None)
        } else {
            let model_info = model_info
                .as_ref()
                .context("this NFIQ 2 build requires an explicit model information file")?;
            ensure!(
                fs::metadata(model_info)?.len() <= 65_536,
                "model information exceeds 64 KiB"
            );
            let info_text = fs::read_to_string(model_info)?;
            let paths: Vec<_> = info_text
                .lines()
                .filter_map(|line| line.split_once('='))
                .filter(|(key, _)| key.trim() == "Path")
                .map(|(_, value)| value.trim())
                .collect();
            ensure!(
                paths.len() == 1 && !paths[0].is_empty(),
                "model information must contain exactly one Path entry"
            );
            let model_data = model_info
                .parent()
                .context("model information has no directory")?
                .join(paths[0]);
            (Some(hash_file(model_info)?), Some(hash_file(&model_data)?))
        };
        let engine = EngineIdentity {
            name: "NFIQ 2",
            version: SUPPORTED_VERSION.into(),
            executable_sha256: hash_file(&executable)?,
            model_source: if embedded {
                "embedded_in_executable"
            } else {
                "external_model_file"
            },
            model_info_sha256,
            model_data_sha256,
        };
        let region = analysis_region(captured);
        let mut pixels = Vec::with_capacity(region.width * region.height);
        for y in region.y..region.y + region.height {
            let start = y * captured.image.width + region.x;
            pixels.extend_from_slice(&captured.image.pixels[start..start + region.width]);
        }
        let mut input = CapturedImage::new(Image {
            width: region.width,
            height: region.height,
            pixels,
        });
        // Encode the already validated 500-PPI value exactly as a PNG declaration.
        input.metadata.resolution = Resolution::declared(500.0, 500.0)?;
        let input_path = temporary.0.join("input.png");
        input.save_png(&input_path)?;
        // -F is safe only after our resolution/type checks; transformation flags
        // in the output must still be zero. Never force unknown-PPI input.
        let mut args = vec!["-v".as_ref(), "-a".as_ref(), "-F".as_ref()];
        if let Some(path) = &model_info {
            args.extend(["-m".as_ref(), path.as_os_str()]);
        }
        args.push(input_path.as_os_str());
        let output = run(&executable, &args, &temporary.0, "score", self.timeout)?;
        let (score, actionable_feedback) = parse_output(&output)?;
        Ok(QualityOutcome::Scored {
            engine,
            score,
            input_region: region,
            evaluation_ppi: 500,
            actionable_feedback,
        })
    }
}

/// The upstream Windows model parser joins paths with '/'. That is incompatible
/// with Rust's verbatim '\\?\' canonical paths, which disable slash normalization.
pub(super) fn canonical_cli_path(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = path.canonicalize()?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let wide: Vec<_> = canonical.as_os_str().encode_wide().collect();
        let unc: Vec<_> = "\\\\?\\UNC\\".encode_utf16().collect();
        let verbatim: Vec<_> = "\\\\?\\".encode_utf16().collect();
        if wide.starts_with(&unc) {
            let mut normal: Vec<_> = "\\\\".encode_utf16().collect();
            normal.extend_from_slice(&wide[unc.len()..]);
            return Ok(std::ffi::OsString::from_wide(&normal).into());
        }
        if wide.starts_with(&verbatim) && wide.get(5) == Some(&(b':' as u16)) {
            return Ok(std::ffi::OsString::from_wide(&wide[verbatim.len()..]).into());
        }
    }
    Ok(canonical)
}

pub(super) fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).context("cannot open engine/model for fingerprinting")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(super) struct Workspace(pub(super) PathBuf);
impl Workspace {
    pub(super) fn new() -> Result<Self> {
        for _ in 0..10 {
            let mut nonce = [0; 16];
            getrandom::fill(&mut nonce)?;
            let path = std::env::temp_dir()
                .join(format!("uareu-quality-{:032x}", u128::from_le_bytes(nonce)));
            #[allow(unused_mut)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("cannot allocate a private quality workspace")
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

pub(super) fn run(
    executable: &Path,
    args: &[&std::ffi::OsStr],
    directory: &Path,
    stage: &str,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let stdout = directory.join(format!("{stage}.stdout"));
    let stderr = directory.join(format!("{stage}.stderr"));
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(private_file(&stdout)?)
        .stderr(private_file(&stderr)?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().context("cannot start quality engine")?;
    let started = Instant::now();
    loop {
        let oversized = [stdout.as_path(), stderr.as_path()]
            .iter()
            .any(|path| fs::metadata(path).map_or(true, |m| m.len() > OUTPUT_LIMIT));
        if oversized || started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!(if oversized {
                "quality engine output exceeded 128 KiB"
            } else {
                "quality engine evaluation timed out"
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let mut error_output = Vec::new();
                    File::open(&stderr)?
                        .take(OUTPUT_LIMIT)
                        .read_to_end(&mut error_output)?;
                    bail!(
                        "quality engine process failed ({status}): {}",
                        String::from_utf8_lossy(&error_output).trim()
                    );
                }
                let mut output = Vec::new();
                File::open(&stdout)?
                    .take(OUTPUT_LIMIT + 1)
                    .read_to_end(&mut output)?;
                ensure!(
                    output.len() as u64 <= OUTPUT_LIMIT
                        && fs::metadata(&stderr)?.len() <= OUTPUT_LIMIT,
                    "quality engine output exceeded 128 KiB"
                );
                return Ok(output);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    }
}

fn parse_output(bytes: &[u8]) -> Result<(u8, BTreeMap<String, f64>)> {
    let text = std::str::from_utf8(bytes).context("NFIQ 2 output is not UTF-8")?;
    let records = csv_records(text)?;
    ensure!(
        records.len() == 2,
        "expected one NFIQ 2 CSV header and one result"
    );
    let header = &records[0];
    let row = &records[1];
    ensure!(
        header.len() == row.len(),
        "NFIQ 2 CSV column count mismatch"
    );
    let value = |name: &str| -> Result<&str> {
        let columns: Vec<_> = header
            .iter()
            .enumerate()
            .filter(|(_, key)| *key == name)
            .collect();
        ensure!(
            columns.len() == 1,
            "missing or duplicate NFIQ 2 column {name}"
        );
        Ok(row[columns[0].0].as_str())
    };
    ensure!(
        value("OptionalError")? == "NA" || value("OptionalError")?.is_empty(),
        "NFIQ 2 could not score input: {}",
        value("OptionalError")?
    );
    ensure!(
        value("Quantized")? == "0" && value("Resampled")? == "0",
        "NFIQ 2 unexpectedly changed image depth or resolution"
    );
    let score: u8 = value("QualityScore")?
        .parse()
        .context("invalid NFIQ 2 score")?;
    ensure!(score <= 100, "NFIQ 2 score is outside 0..100");
    let mut feedback = BTreeMap::new();
    for name in [
        "UniformImage",
        "EmptyImageOrContrastTooLow",
        "FingerprintImageWithMinutiae",
        "SufficientFingerprintForeground",
    ] {
        let number: f64 = value(name)?
            .parse()
            .context("invalid NFIQ 2 actionable value")?;
        ensure!(number.is_finite(), "non-finite NFIQ 2 actionable value");
        feedback.insert(name.into(), number);
    }
    Ok((score, feedback))
}

/// Strict CSV subset with quoted commas, doubled quotes and multiline errors.
fn csv_records(text: &str) -> Result<Vec<Vec<String>>> {
    let mut records = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut closed = false;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                    closed = true;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !closed => quoted = true,
            ',' => {
                row.push(std::mem::take(&mut field));
                closed = false;
            }
            '\n' | '\r' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut row));
                closed = false;
            }
            _ => {
                ensure!(!closed && c != '"', "malformed NFIQ 2 CSV quoting");
                field.push(c);
            }
        }
    }
    ensure!(!quoted, "unterminated NFIQ 2 CSV quote");
    if closed || !field.is_empty() || !row.is_empty() {
        row.push(field);
        records.push(row);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "Filename,QualityScore,OptionalError,Quantized,Resampled,UniformImage,EmptyImageOrContrastTooLow,FingerprintImageWithMinutiae,SufficientFingerprintForeground\n";

    #[test]
    fn parses_reference_schema_and_rejects_invalid_results() {
        let good = format!("{HEADER}\"a,b.png\",90,NA,0,0,35.6,163.2,55,98722\n");
        assert_eq!(parse_output(good.as_bytes()).unwrap().0, 90);
        assert_eq!(
            parse_output(good.replace(",90,", ",0,").as_bytes())
                .unwrap()
                .0,
            0
        );
        for bad in [
            good.replace(",90,", ",101,"),
            good.replace(",NA,", ",\"bad\ninput\","),
            good.replace(",0,0,", ",0,1,"),
            good.replace(",35.6,", ",NaN,"),
            format!("{good}extra\n"),
            good.replace(",90,", ",NA,"),
        ] {
            assert!(parse_output(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(csv_records("\"unclosed").is_err());
        assert!(csv_records("\"closed\"garbage").is_err());
    }

    #[test]
    fn external_process_contract_handles_failures_deadlines_and_model_identity() {
        let build = Workspace::new().unwrap();
        let binary = build
            .0
            .join(format!("stub{}", std::env::consts::EXE_SUFFIX));
        let compilation = Command::new("rustc")
            .args(["--edition=2021", "--crate-name", "nfiq2_process_fixture"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/nfiq2_stub.rs"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            compilation.status.success(),
            "{}",
            String::from_utf8_lossy(&compilation.stderr)
        );
        let mut captured = CapturedImage::new(Image {
            width: 384,
            height: 290,
            pixels: vec![128; 384 * 290],
        });
        captured.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
        for mode in [
            "success",
            "crop",
            "embedded",
            "version",
            "badscore",
            "resampled",
            "nonzero",
            "flood",
            "hang",
        ] {
            let dir = Workspace::new().unwrap();
            let executable = dir
                .0
                .join(format!("engine{}", std::env::consts::EXE_SUFFIX));
            fs::copy(&binary, &executable).unwrap();
            fs::write(dir.0.join("mode.txt"), mode).unwrap();
            let model_info = dir.0.join("model.txt");
            fs::write(&model_info, "Name = process fixture\nPath = model.yaml\n").unwrap();
            fs::write(dir.0.join("model.yaml"), "not a real model").unwrap();
            let mut engine = Nfiq2 {
                executable,
                model_info: if mode == "embedded" {
                    None
                } else {
                    Some(model_info.clone())
                },
                timeout: Duration::from_secs(3),
            };
            if mode == "hang" {
                engine.timeout = Duration::from_millis(500);
            }
            captured.metadata.valid_region = if mode == "crop" {
                Some(crate::metadata::Region {
                    x: 20,
                    y: 30,
                    width: 200,
                    height: 210,
                })
            } else {
                None
            };
            let started = Instant::now();
            let outcome = engine.assess(&captured);
            match (mode, &outcome) {
                ("success" | "crop" | "embedded", QualityOutcome::Scored { score, engine, .. }) => {
                    assert_eq!(*score, 90);
                    assert_eq!(engine.executable_sha256.len(), 64);
                    assert_eq!(engine.model_data_sha256.is_none(), mode == "embedded");
                }
                ("success" | "crop" | "embedded", _) => panic!("{mode}: {outcome:?}"),
                (_, QualityOutcome::EvaluationFailed { reason }) => {
                    if mode == "hang" {
                        assert!(reason.contains("timed out"), "{reason}");
                        assert!(started.elapsed() < Duration::from_secs(10));
                    }
                }
                _ => panic!("{mode}: {outcome:?}"),
            }
            if let Ok(path) = fs::read_to_string(dir.0.join("observed_path.txt")) {
                assert!(
                    !Path::new(&path).exists(),
                    "temporary image must be removed on every outcome"
                );
            }
            if mode == "embedded" {
                engine.model_info = Some(model_info);
                assert!(
                    matches!(engine.assess(&captured), QualityOutcome::EvaluationFailed { reason } if reason.contains("ignore -m"))
                );
            }
        }
    }
}
