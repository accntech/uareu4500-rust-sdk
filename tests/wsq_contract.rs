// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic external-process contract checks; never NBIS validation.
#[path = "support/reference_process.rs"]
mod process;
use std::{fs, path::PathBuf, process::Command, time::Duration};
use uareu::{metadata::Resolution, wsq::WsqCodec};
#[test]
fn decoded_nistcom_variants_and_external_failures_are_checked() {
    let w = process::Workspace::new().unwrap();
    let executable = w.0.join(if cfg!(windows) { "codec.exe" } else { "codec" });
    assert!(
        Command::new("rustc")
            .args(["--edition=2024", "tests/support/wsq_stub.rs", "-o"])
            .arg(&executable)
            .status()
            .unwrap()
            .success()
    );
    let make = |timeout| WsqCodec {
        encoder: PathBuf::from("absent"),
        decoder: executable.clone(),
        timeout,
    };
    for (mode, ok) in [
        ("normal", true),
        ("unknown_ppi", true),
        ("duplicate", false),
        ("short_pixels", false),
        ("extreme_ppi", false),
        ("failure", false),
        ("hang", false),
    ] {
        fs::write(w.0.join("mode"), mode).unwrap();
        // Leave room for process startup under build load; only the hang case
        // needs a deliberately short deadline.
        let timeout = if mode == "hang" {
            Duration::from_millis(300)
        } else {
            Duration::from_secs(5)
        };
        let result = make(timeout).decode(&[0xff, 0xa0, 0xff, 0xa1]);
        assert_eq!(result.is_ok(), ok, "mode={mode}: {result:?}");
        if let Ok(image) = result {
            assert_eq!((image.image.width, image.image.height), (2, 3));
            assert_eq!(image.image.pixels, [1, 2, 3, 4, 5, 6]);
            if mode == "unknown_ppi" {
                assert!(matches!(image.metadata.resolution, Resolution::Unknown));
            }
        }
    }
    fs::write(w.0.join("mode"), "failure").unwrap();
    assert!(
        process::run(
            &executable,
            &["raw".into(), w.0.join("input.wsq").into_os_string()],
            &w.0,
            Duration::from_millis(300)
        )
        .is_err()
    );
}

#[test]
fn target_size_is_measured_and_search_is_bounded() {
    use uareu::{
        frame::Image,
        metadata::CapturedImage,
        wsq::{WsqTargetOptions, WsqTargetStatus},
    };
    let w = process::Workspace::new().unwrap();
    let executable = w.0.join(if cfg!(windows) { "codec.exe" } else { "codec" });
    assert!(
        Command::new("rustc")
            .args(["--edition=2024", "tests/support/wsq_stub.rs", "-o"])
            .arg(&executable)
            .status()
            .unwrap()
            .success()
    );
    let codec = WsqCodec {
        encoder: executable.clone(),
        decoder: executable,
        timeout: Duration::from_secs(2),
    };
    let mut image = CapturedImage::new(Image {
        width: 256,
        height: 256,
        pixels: vec![128; 65536],
    });
    image.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
    let options = WsqTargetOptions {
        target_bytes: 1000,
        max_encodes: 8,
        deadline: Duration::from_secs(2),
    };
    fs::write(w.0.join("mode"), "encode").unwrap();
    let result = codec.encode_to_target(&image, options).unwrap();
    assert_eq!(result.status, WsqTargetStatus::Met);
    assert!(result.bytes.len() <= 1000);
    assert!(result.encodes <= 8);
    // Minimum allowed bitrate still exceeds this target; report the measured result.
    let result = codec
        .encode_to_target(
            &image,
            WsqTargetOptions {
                target_bytes: 1,
                ..options
            },
        )
        .unwrap();
    assert_eq!(result.status, WsqTargetStatus::Unattainable);
    assert_eq!(result.encodes, 2);
    assert_eq!(result.bytes.len(), 200);
    // A single attempt must retain its valid oversized candidate.
    let result = codec
        .encode_to_target(
            &image,
            WsqTargetOptions {
                max_encodes: 1,
                ..options
            },
        )
        .unwrap();
    assert_eq!(result.status, WsqTargetStatus::EncodeLimit);
    assert_eq!(result.encodes, 1);
    assert_eq!(result.bytes.len(), 2350);
    // A later oversized trial must retain the previously sampled fitting output.
    let result = codec
        .encode_to_target(
            &image,
            WsqTargetOptions {
                max_encodes: 3,
                ..options
            },
        )
        .unwrap();
    assert_eq!(result.encodes, 3);
    assert_eq!(result.status, WsqTargetStatus::Met);
    assert_eq!(result.bytes.len(), 200);
    assert_eq!(result.bitrate, 0.1);
    // A later codec failure is observable and must not erase the valid result.
    fs::write(w.0.join("mode"), "encode_then_fail").unwrap();
    let result = codec.encode_to_target(&image, options).unwrap();
    assert!(matches!(result.status, WsqTargetStatus::CodecFailure(_)));
    assert_eq!(result.encodes, 2);
    assert_eq!(result.bytes.len(), 2350);
    // Subsequent attempts hang: combined deadline must preserve the prior candidate.
    fs::write(w.0.join("mode"), "encode_then_hang").unwrap();
    let start = std::time::Instant::now();
    let result = codec
        .encode_to_target(
            &image,
            WsqTargetOptions {
                deadline: Duration::from_millis(250),
                ..options
            },
        )
        .unwrap();
    assert_eq!(result.status, WsqTargetStatus::Deadline);
    assert_eq!(result.encodes, 2);
    assert_eq!(result.bytes.len(), 2350);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        codec
            .encode_to_target(
                &image,
                WsqTargetOptions {
                    max_encodes: 0,
                    ..options
                }
            )
            .is_err()
    );
    assert!(
        codec
            .encode_to_target(
                &image,
                WsqTargetOptions {
                    deadline: Duration::ZERO,
                    ..options
                }
            )
            .is_err()
    );
}
