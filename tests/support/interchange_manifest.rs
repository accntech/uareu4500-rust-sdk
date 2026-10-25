// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
#[path = "reference_process.rs"]
mod process;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    producer: String,
    vectors: Vec<Vector>,
    consumer: Consumer,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Consumer {
    executable: PathBuf,
    args: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Vector {
    name: String,
    format: String,
    path: PathBuf,
    sha256: String,
    valid: bool,
    expected: Value,
    source: String,
    license: String,
}
fn parse(value: &Value) -> Result<Manifest> {
    let m: Manifest = serde_json::from_value(value.clone())?;
    ensure!(
        m.schema_version == 1
            && !m.producer.trim().is_empty()
            && !m.producer.to_lowercase().contains("uareu"),
        "independent producer required"
    );
    ensure!(
        !m.vectors.is_empty() && m.vectors.len() <= 1000,
        "reference vectors required (maximum 1000)"
    );
    ensure!(
        !m.consumer.executable.as_os_str().is_empty(),
        "independent consumer required"
    );
    let mut names = std::collections::BTreeSet::new();
    for v in &m.vectors {
        ensure!(
            !v.name.trim().is_empty() && names.insert(&v.name),
            "unique vector names required"
        );
        ensure!(
            !v.source.trim().is_empty() && !v.license.trim().is_empty(),
            "fixture provenance/permission required"
        );
        ensure!(
            v.sha256.len() == 64 && v.sha256.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid fixture digest"
        );
        ensure!(
            matches!(
                v.format.as_str(),
                "ansi378_2004" | "iso19794_2_2005" | "iso19794_4_2005"
            ),
            "unsupported reference edition"
        );
        ensure!(v.expected.is_object(), "expected fields must be an object");
    }
    Ok(m)
}
pub fn validate_manifest(value: &Value) -> Result<()> {
    parse(value).map(|_| ())
}
fn decoded(bytes: &[u8], format: &str) -> Result<(Value, Vec<u8>)> {
    if format == "iso19794_4_2005" {
        let d = uareu::image_record::decode(bytes)?;
        let fields = json!({"width":d.image.image.width,"height":d.image.image.height,"pixels_sha256":format!("{:x}",Sha256::digest(&d.image.image.pixels)),"resolution":d.image.metadata.resolution,"options":d.options});
        let encoded = uareu::image_record::encode(&d.image, &d.options)?;
        Ok((fields, encoded))
    } else {
        let f = if format == "ansi378_2004" {
            uareu::template_io::TemplateFormat::Ansi378_2004
        } else {
            uareu::template_io::TemplateFormat::Iso19794_2_2005
        };
        let d = uareu::template_io::decode(bytes, f)?;
        Ok((
            serde_json::to_value(&d)?,
            uareu::template_io::encode(&d, f)?,
        ))
    }
}
fn bytes(path: &Path, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        fs::metadata(path)?.len() <= limit as u64,
        "reference input too large"
    );
    let mut b = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut b)?;
    ensure!(b.len() <= limit, "reference input grew beyond limit");
    Ok(b)
}
pub fn run_from_environment() -> Result<()> {
    let path = PathBuf::from(
        std::env::var_os("UAREU_INTERCHANGE_MANIFEST")
            .context("set UAREU_INTERCHANGE_MANIFEST; no independent input is bundled")?,
    );
    let value: Value = serde_json::from_slice(&bytes(&path, 1024 * 1024)?)?;
    let m = parse(&value)?;
    let root = fs::canonicalize(path.parent().unwrap_or(Path::new(".")))?;
    let exe = if m.consumer.executable.is_absolute() {
        m.consumer.executable.clone()
    } else {
        root.join(&m.consumer.executable)
    };
    for v in m.vectors {
        let input = bytes(&root.join(&v.path), 4 * 1024 * 1024)?;
        ensure!(
            format!("{:x}", Sha256::digest(&input)).eq_ignore_ascii_case(&v.sha256),
            "fixture hash mismatch for {}",
            v.name
        );
        let result = decoded(&input, &v.format);
        ensure!(
            result.is_ok() == v.valid,
            "validity mismatch for {}",
            v.name
        );
        let (expected, output) = if v.valid {
            let (fields, encoded) = result?;
            ensure!(
                fields == v.expected,
                "expected parsed fields differ for {}",
                v.name
            );
            (fields, encoded)
        } else {
            (json!({}), input)
        };
        let temp = process::Workspace::new()?;
        let exported = temp.0.join("record.bin");
        fs::write(&exported, &output)?;
        let mut args: Vec<OsString> = m.consumer.args.iter().map(OsString::from).collect();
        args.push(v.format.clone().into());
        args.push(exported.into_os_string());
        let receipt: Value = serde_json::from_slice(&process::run(
            &exe,
            &args,
            &temp.0,
            Duration::from_secs(30),
        )?)?;
        ensure!(
            receipt["accepted"].as_bool() == Some(v.valid),
            "consumer validity differs for {}",
            v.name
        );
        ensure!(
            receipt["sha256"] == format!("{:x}", Sha256::digest(&output)),
            "consumer receipt not bound to exported bytes"
        );
        if v.valid {
            ensure!(
                receipt["fields"] == expected,
                "consumer fields differ for {}",
                v.name
            );
        }
        println!(
            "{}",
            json!({"vector":v.name,"format":v.format,"status":"passed","sha256":format!("{:x}",Sha256::digest(&output)),"producer":m.producer})
        );
    }
    Ok(())
}
