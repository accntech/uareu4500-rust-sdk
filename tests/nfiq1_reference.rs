// SPDX-License-Identifier: GPL-3.0-or-later
#[path = "support/reference_process.rs"]
mod process;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};
use uareu::{
    frame::Image,
    metadata::{CapturedImage, Resolution},
    quality::{LegacyNfiqOutcome, Nfiq1},
};

#[test]
#[ignore = "requires NBIS5 NFIQ1 executable and official NIST synthetic PGM"]
fn legacy_nfiq_cli_and_adapter_agree_on_official_synthetic_pixels() -> Result<()> {
    let executable =
        PathBuf::from(std::env::var_os("UAREU_NFIQ1_EXE").context("set UAREU_NFIQ1_EXE")?);
    let input = PathBuf::from(
        std::env::var_os("UAREU_NFIQ1_TEST_PGM")
            .context("set UAREU_NFIQ1_TEST_PGM to official SFinGe_Test01.pgm")?,
    );
    ensure!(
        fs::metadata(&input)?.len() <= 4 * 1024 * 1024,
        "oversized fixture"
    );
    let bytes = fs::read(input)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    ensure!(
        digest == "827e1485f6cd267dc280cf2f30af19759764ff421d62c3396ffe14738553faa9",
        "expected exact official synthetic fixture"
    );
    let mut offset = 0;
    let mut tokens = vec![];
    while tokens.len() < 4 {
        while bytes.get(offset).is_some_and(u8::is_ascii_whitespace) {
            offset += 1;
        }
        if bytes.get(offset) == Some(&b'#') {
            while bytes.get(offset).is_some_and(|c| *c != b'\n') {
                offset += 1;
            }
            continue;
        }
        let start = offset;
        while bytes.get(offset).is_some_and(|c| !c.is_ascii_whitespace()) {
            offset += 1;
        }
        ensure!(offset > start, "missing PGM token");
        tokens.push(std::str::from_utf8(&bytes[start..offset])?.to_owned());
    }
    ensure!(tokens[0] == "P5" && tokens[3] == "255", "wrong PGM format");
    if bytes.get(offset..offset + 2) == Some(b"\r\n") {
        offset += 2;
    } else {
        offset += 1;
    }
    let width: usize = tokens[1].parse()?;
    let height: usize = tokens[2].parse()?;
    let pixels = bytes[offset..].to_vec();
    ensure!(pixels.len() == width * height, "wrong PGM size");
    let mut captured = CapturedImage::new(Image {
        width,
        height,
        pixels,
    });
    captured.metadata.resolution = Resolution::declared(500.0, 500.0)?;
    let original = captured.clone();
    let engine = Nfiq1 {
        executable: executable.clone(),
        timeout: Duration::from_secs(30),
    };
    let outcome = engine.assess(&captured);
    let LegacyNfiqOutcome::Scored {
        score,
        engine: identity,
        ..
    } = outcome
    else {
        anyhow::bail!("legacy backend failed: {outcome:?}");
    };
    let workspace = process::Workspace::new()?;
    let raw = workspace.0.join("reference.raw");
    fs::write(&raw, &captured.image.pixels)?;
    let geometry = format!("{width},{height},8,500");
    let direct = process::run(
        &executable,
        &[
            "-o".into(),
            "-raw".into(),
            geometry.into(),
            raw.into_os_string(),
        ],
        &workspace.0,
        Duration::from_secs(30),
    )?;
    let direct: u8 = std::str::from_utf8(&direct)?.trim().parse()?;
    ensure!(
        score == direct && (1..=5).contains(&score),
        "legacy adapter/direct mismatch"
    );
    assert_eq!(captured, original);
    println!(
        "{}",
        serde_json::json!({"status":"passed","engine":"NFIQ1 NBIS5","score":score,"direction":"lower_is_better_1_to_5","executable_sha256":identity.executable_sha256,"fixture_sha256":digest,"pixels_sha256":format!("{:x}",Sha256::digest(&captured.image.pixels)),"limitation":"same backend adapter agreement on official synthetic declared500PPI; not vendor parity or capture DPI"})
    );
    Ok(())
}

#[test]
#[ignore = "requires NBIS5 NFIQ1 executable"]
fn explicit_raw_pixels_cannot_be_misdetected_as_an_ansi_nist_file() -> Result<()> {
    let executable =
        PathBuf::from(std::env::var_os("UAREU_NFIQ1_EXE").context("set UAREU_NFIQ1_EXE")?);
    let mut pixels = vec![128; 256 * 256];
    pixels[..6].copy_from_slice(b"1.001:");
    let mut captured = CapturedImage::new(Image {
        width: 256,
        height: 256,
        pixels,
    });
    captured.metadata.resolution = Resolution::declared(500.0, 500.0)?;
    let result = Nfiq1 {
        executable,
        timeout: Duration::from_secs(30),
    }
    .assess(&captured);
    ensure!(
        matches!(result, LegacyNfiqOutcome::Scored { score: 1..=5, .. }),
        "raw format misdetected: {result:?}"
    );
    Ok(())
}
