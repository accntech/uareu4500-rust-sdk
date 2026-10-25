// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in real-engine check; test images and executables are never bundled.
use std::{fs, path::PathBuf, process::Command, time::Duration};
use uareu4500::{
    frame::Image,
    metadata::{CapturedImage, Resolution},
    quality::{Nfiq2, QualityOutcome},
};

#[test]
#[ignore = "requires NFIQ 2 v2.3.0 and an explicitly provided synthetic 500-PPI P5 PGM"]
fn reference_cli_and_adapter_agree_on_identical_synthetic_pixels() {
    let executable =
        PathBuf::from(std::env::var_os("UAREU_NFIQ2_EXE").expect("set UAREU_NFIQ2_EXE"));
    let model_info = std::env::var_os("UAREU_NFIQ2_MODEL").map(PathBuf::from);
    let input = PathBuf::from(
        std::env::var_os("UAREU_NFIQ2_TEST_PGM")
            .expect("set UAREU_NFIQ2_TEST_PGM to a synthetic 500-PPI fixture"),
    );
    let bytes = fs::read(&input).unwrap();
    let mut offset = 0;
    let mut token = || {
        loop {
            while bytes[offset].is_ascii_whitespace() {
                offset += 1;
            }
            if bytes[offset] != b'#' {
                break;
            }
            while bytes[offset] != b'\n' {
                offset += 1;
            }
        }
        let start = offset;
        while !bytes[offset].is_ascii_whitespace() {
            offset += 1;
        }
        std::str::from_utf8(&bytes[start..offset])
            .unwrap()
            .to_owned()
    };
    assert_eq!(token(), "P5");
    let width: usize = token().parse().unwrap();
    let height: usize = token().parse().unwrap();
    assert_eq!(token(), "255");
    if bytes[offset] == b'\r' && bytes.get(offset + 1) == Some(&b'\n') {
        offset += 2;
    } else {
        offset += 1;
    }
    assert_eq!(bytes.len() - offset, width * height);
    let mut captured = CapturedImage::new(Image {
        width,
        height,
        pixels: bytes[offset..].to_vec(),
    });
    // Explicit property of this test input, never a default for reader captures.
    captured.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
    let engine = Nfiq2 {
        executable: executable.clone(),
        model_info: model_info.clone(),
        timeout: Duration::from_secs(30),
    };
    let outcome = engine.assess(&captured);
    let QualityOutcome::Scored { score, .. } = outcome else {
        panic!("{outcome:?}");
    };
    let mut command = Command::new(executable);
    command.arg("-F");
    if let Some(model) = model_info {
        command.arg("-m").arg(model);
    }
    let reference = command.arg(input).output().unwrap();
    assert!(
        reference.status.success(),
        "{}",
        String::from_utf8_lossy(&reference.stderr)
    );
    let reference_score: u8 = String::from_utf8(reference.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(score, reference_score);
    eprintln!("reference CLI and adapter agree: {score}");
}
