// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};
use uareu::metadata::{CapturedImage, Resolution};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub engine_path: String,
    pub timeout_ms: u64,
    pub seed: u64,
    pub data_use_authority: String,
    pub samples: Vec<Sample>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub subject_id: String,
    pub finger_id: String,
    pub session_id: String,
    pub split: String,
    pub role: String,
    pub path: String,
    pub sha256: String,
    pub scale_evidence: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScaleEvidence {
    schema_version: u32,
    image_sha256: String,
    x_ppi: f64,
    y_ppi: f64,
    justification: String,
}
pub fn validate_structure(manifest: &Manifest) -> Result<()> {
    ensure!(manifest.schema_version == 1, "unsupported manifest schema");
    ensure!(
        !manifest.engine_path.trim().is_empty(),
        "engine path is required"
    );
    ensure!(
        !manifest.data_use_authority.trim().is_empty(),
        "data-use authority is required"
    );
    ensure!(
        (1..=300_000).contains(&manifest.timeout_ms),
        "invalid operation timeout"
    );
    ensure!(
        !manifest.samples.is_empty() && manifest.samples.len() <= 2000,
        "evaluation supports 1..2000 samples"
    );
    let mut subjects = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    type SessionRoles<'a> = (BTreeSet<&'a str>, BTreeSet<&'a str>);
    let mut sessions: BTreeMap<(&str, &str), SessionRoles<'_>> = BTreeMap::new();
    for sample in &manifest.samples {
        for id in [&sample.subject_id, &sample.finger_id, &sample.session_id] {
            ensure!(
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|v| v.is_ascii_alphanumeric() || b"_-".contains(&v)),
                "IDs must be opaque ASCII identifiers"
            );
        }
        ensure!(
            ["development", "evaluation"].contains(&sample.split.as_str()),
            "invalid split"
        );
        ensure!(
            ["enrollment", "probe"].contains(&sample.role.as_str()),
            "invalid role"
        );
        if let Some(old) = subjects.insert(&sample.subject_id, &sample.split) {
            ensure!(old == &sample.split, "subject leaks across splits");
        }
        ensure!(
            sample.sha256.len() == 64
                && sample
                    .sha256
                    .bytes()
                    .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v)),
            "image hash must be lowercase SHA-256"
        );
        if let Some(old) = hashes.insert(
            &sample.sha256,
            (
                &sample.split,
                &sample.role,
                &sample.subject_id,
                &sample.finger_id,
            ),
        ) {
            ensure!(
                old == (
                    &sample.split,
                    &sample.role,
                    &sample.subject_id,
                    &sample.finger_id
                ),
                "image hash leaks across splits/roles/identities"
            );
            anyhow::bail!("duplicate image hash; repeated copies cannot add samples");
        }
        ensure!(
            !sample.path.trim().is_empty() && !sample.scale_evidence.trim().is_empty(),
            "image path and scale evidence required"
        );
        let entry = sessions
            .entry((&sample.subject_id, &sample.finger_id))
            .or_default();
        if sample.role == "enrollment" {
            entry.0.insert(&sample.session_id);
        } else {
            entry.1.insert(&sample.session_id);
        }
    }
    for split in ["development", "evaluation"] {
        ensure!(
            subjects
                .values()
                .filter(|value| value.as_str() == split)
                .count()
                >= 2,
            "each split requires at least two distinct subjects"
        );
    }
    for (enrollment, probe) in sessions.values() {
        ensure!(
            !enrollment.is_empty() && !probe.is_empty(),
            "every finger needs enrollment and probe samples"
        );
        ensure!(
            enrollment.is_disjoint(probe),
            "enrollment/probe sessions overlap"
        );
    }
    Ok(())
}
pub fn resolve(root: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}
pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "input exceeds size limit");
    Ok(bytes)
}
pub fn load(path: &Path) -> Result<(Manifest, PathBuf, String)> {
    let bytes = read_bounded(path, 16 * 1024 * 1024)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let mut manifest: Manifest = serde_json::from_slice(&bytes).context("invalid manifest")?;
    validate_structure(&manifest)?;
    let root = path.parent().unwrap_or_else(|| Path::new(".")).to_owned();
    ensure!(
        !read_bounded(&resolve(&root, &manifest.data_use_authority), 1024 * 1024)?.is_empty(),
        "data-use authority record is empty"
    );
    for sample in &manifest.samples {
        let _ = image(&root, sample)?;
    }
    manifest.samples.sort_by(|a, b| {
        (&a.subject_id, &a.finger_id, &a.session_id, &a.sha256).cmp(&(
            &b.subject_id,
            &b.finger_id,
            &b.session_id,
            &b.sha256,
        ))
    });
    Ok((manifest, root, digest))
}
pub fn image(root: &Path, sample: &Sample) -> Result<CapturedImage> {
    let path = resolve(root, &sample.path);
    let bytes = read_bounded(&path, 32 * 1024 * 1024)?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == sample.sha256,
        "image hash mismatch"
    );
    // Read through the product's PNG loader; the evidence never overrides its metadata.
    let captured = CapturedImage::load_png(&path)?;
    let evidence: ScaleEvidence = serde_json::from_slice(&read_bounded(
        &resolve(root, &sample.scale_evidence),
        64 * 1024,
    )?)?;
    ensure!(
        evidence.schema_version == 1
            && evidence.image_sha256 == sample.sha256
            && !evidence.justification.trim().is_empty(),
        "scale evidence does not identify this image or justify its scale"
    );
    let Resolution::Known { x_ppi, y_ppi, .. } = captured.metadata.resolution else {
        anyhow::bail!("image scale is unknown");
    };
    ensure!(
        evidence.x_ppi.is_finite()
            && evidence.y_ppi.is_finite()
            && (x_ppi - evidence.x_ppi).abs() <= 0.0128
            && (y_ppi - evidence.y_ppi).abs() <= 0.0128,
        "scale evidence conflicts with PNG metadata"
    );
    ensure!(
        (x_ppi - y_ppi).abs() < 0.0128 && (250.0..=2000.0).contains(&x_ppi),
        "unsupported recognition scale"
    );
    Ok(captured)
}
