// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in bidirectional interchange against independently implemented FingerprintIO.
#[path = "support/reference_process.rs"]
mod process;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use uareu::template_io::{
    self, FingerprintView, Minutia, MinutiaeRecord, MultiViewRecord, TemplateFormat,
};

fn pinned_classpath() -> Result<OsString> {
    let classpath = std::env::var_os("UAREU_FINGERPRINTIO_CLASSPATH")
        .context("set UAREU_FINGERPRINTIO_CLASSPATH")?;
    let pins = [
        (
            "fingerprintio-1.3.3.jar",
            "daebcf2dd68726c543e334b16fd09d6a7aa709ff0e807e643c251697fd784ca7",
        ),
        (
            "stagean-1.3.0.jar",
            "19a77e39c6db01950c6caedc72323995f4be38368a16f72ffce5ea49ef3c7650",
        ),
        (
            "noexception-1.9.1.jar",
            "29b002c8ccb17e73f06e2a237b083a38ca8e83cf833e0483c2366ff492965a8d",
        ),
        (
            "closeablescope-1.0.1.jar",
            "3e3554e56a831a68e168ee0e267b7e5ede9a9623c5814065888de49de0debfc8",
        ),
        (
            "slf4j-api-2.0.9.jar",
            "0818930dc8d7debb403204611691da58e49d42c50b6ffcfdce02dadb7c3c2b6c",
        ),
    ];
    let paths: Vec<_> = std::env::split_paths(&classpath).collect();
    ensure!(
        paths.len() == pins.len(),
        "classpath must contain exactly the five pinned jars"
    );
    for (name, hash) in pins {
        let path = paths
            .iter()
            .find(|p| p.file_name().is_some_and(|f| f == name))
            .with_context(|| format!("missing {name}"))?;
        ensure!(
            format!("{:x}", Sha256::digest(fs::read(path)?)) == hash,
            "jar hash mismatch: {name}"
        );
    }
    Ok(classpath)
}
fn single(record: &MultiViewRecord) -> MinutiaeRecord {
    let view = &record.views[0];
    MinutiaeRecord {
        width: record.width,
        height: record.height,
        pixels_per_cm_x: record.pixels_per_cm_x,
        pixels_per_cm_y: record.pixels_per_cm_y,
        vendor_id: record.vendor_id,
        subformat: record.subformat,
        device_info: record.device_info,
        finger_position: view.finger_position,
        impression: view.impression,
        quality: view.quality,
        minutiae: view.minutiae.clone(),
    }
}
fn rust_record(count: usize) -> MultiViewRecord {
    MultiViewRecord {
        width: 640,
        height: 480,
        pixels_per_cm_x: 197,
        pixels_per_cm_y: 198,
        vendor_id: 0x103,
        subformat: 0x456,
        device_info: 0x8123,
        views: (0..count)
            .map(|index| FingerprintView {
                finger_position: if index == 1 { 7 } else { 2 },
                view_number: if index == 2 { 1 } else { 0 },
                impression: if index == 1 { 8 } else { 1 },
                quality: 81 + index as u8,
                minutiae: (0..3)
                    .map(|i| Minutia {
                        x: 123 + i * 31 + index as u16,
                        y: 201 + i * 23,
                        kind: i as u8,
                        direction_clockwise: 0.31 + i as f64 * 1.28,
                        quality: 72 + i as u8,
                    })
                    .collect(),
            })
            .collect(),
    }
}
#[test]
#[ignore = "requires pinned UAREU_FINGERPRINTIO_CLASSPATH and a JDK; runs independent FingerprintIO"]
fn fingerprintio_bidirectional_single_and_multiview_interchange() -> Result<()> {
    let classpath = pinned_classpath()?;
    let workspace = process::Workspace::new()?;
    let glue = workspace.0.join("FingerprintIoProbe.java");
    fs::copy("tests/support/FingerprintIoProbe.java", &glue)
        .context("independent Java glue must exist")?;
    let javac =
        PathBuf::from(std::env::var_os("UAREU_JAVAC_EXE").unwrap_or_else(|| "javac".into()));
    let java = PathBuf::from(std::env::var_os("UAREU_JAVA_EXE").unwrap_or_else(|| "java".into()));
    process::run(
        &javac,
        &[
            "-cp".into(),
            classpath.clone(),
            "-d".into(),
            workspace.0.clone().into_os_string(),
            glue.into_os_string(),
        ],
        &workspace.0,
        Duration::from_secs(30),
    )?;
    let mut runtime_paths: Vec<_> = std::env::split_paths(&classpath).collect();
    runtime_paths.push(workspace.0.clone());
    let runtime_cp = std::env::join_paths(runtime_paths)?;
    let probe =
        |mode: &str, format: &str, input: &Path, output: &Path, count: usize| -> Result<Vec<u8>> {
            process::run(
                &java,
                &[
                    "-cp".into(),
                    runtime_cp.clone(),
                    "FingerprintIoProbe".into(),
                    mode.into(),
                    format.into(),
                    input.as_os_str().to_owned(),
                    output.as_os_str().to_owned(),
                    count.to_string().into(),
                ],
                &workspace.0,
                Duration::from_secs(30),
            )
        };
    for (name, format) in [
        ("ansi", TemplateFormat::Ansi378_2004),
        ("iso", TemplateFormat::Iso19794_2_2005),
    ] {
        for count in [1, 3] {
            let fixture = workspace.0.join(format!("java-{name}-{count}.fmr"));
            probe("produce", name, &fixture, &fixture, count)?;
            let generated = fs::read(&fixture)?;
            let parsed = template_io::decode_views(&generated, format)?;
            ensure!(
                parsed.width == 640
                    && parsed.height == 480
                    && parsed.device_info == 0x8123
                    && parsed.pixels_per_cm_x == 197
                    && parsed.pixels_per_cm_y == 198,
                "Java header fields changed"
            );
            ensure!(
                parsed.views.len() == count && parsed.views[0].minutiae.len() == 3,
                "Java view fields changed"
            );
            ensure!(
                parsed.views[0].minutiae[1].x == 60
                    && parsed.views[0].minutiae[1].kind == 1
                    && parsed.views[0].minutiae[1].quality == 71,
                "Java minutiae fields changed"
            );
            ensure!(
                template_io::encode_views(&parsed, format)? == generated,
                "Rust altered Java-generated record"
            );
            if count == 1 {
                ensure!(
                    template_io::decode(&generated, format)?.minutiae.len() == 3,
                    "legacy Java fixture import failed"
                );
            }
            for prefix in 0..generated.len() {
                ensure!(
                    template_io::decode_views(&generated[..prefix], format).is_err(),
                    "truncation accepted by Rust"
                );
            }
            let mut malformed = generated.clone();
            let header = if name == "ansi" { 26 } else { 24 };
            malformed[header + 4] = 0xc0;
            ensure!(
                template_io::decode_views(&malformed, format).is_err(),
                "Rust accepted reserved minutia type"
            );
            if count == 3 {
                ensure!(
                    parsed.views[1].finger_position == 7
                        && parsed.views[1].impression == 8
                        && parsed.views[2].finger_position == 2
                        && parsed.views[2].view_number == 1,
                    "Java multi-view identity/metadata changed"
                );
            }
            let typed = probe("reject-truncations", name, &fixture, &fixture, count)?;
            ensure!(
                String::from_utf8(typed)?.contains("TemplateFormatException"),
                "Java did not report typed rejection"
            );
            let ours = rust_record(count);
            let exported = if count == 1 {
                template_io::encode(&single(&ours), format)?
            } else {
                template_io::encode_views(&ours, format)?
            };
            let input = workspace.0.join(format!("rust-{name}-{count}.fmr"));
            let output = workspace.0.join(format!("accepted-{name}-{count}.fmr"));
            fs::write(&input, &exported)?;
            probe("consume", name, &input, &output, count)?;
            ensure!(
                fs::read(&output)? == exported,
                "FingerprintIO altered Rust export"
            );
            println!(
                "FingerprintIO 1.3.3 {name}: {count} views, Java->Rust and Rust->Java passed; {} truncated prefixes rejected",
                generated.len()
            );
        }
    }
    Ok(())
}
