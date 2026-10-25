// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic process contract tests, not JPEG2000 conformance.
#[allow(dead_code)]
#[path = "support/reference_process.rs"]
mod process;
use std::{fs, process::Command, time::Duration};
use uareu::{
    frame::Image,
    jpeg2000::Jpeg2000Codec,
    metadata::{CapturedImage, ProcessingStep, Resolution},
};
fn codestream(width: u32, height: u32) -> Vec<u8> {
    let mut b = vec![0xff, 0x4f, 0xff, 0x51, 0, 41, 0, 0];
    for v in [width, height, 0, 0, width, height, 0, 0] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b.extend_from_slice(&[0, 1, 7, 1, 1, 0xff, 0xd9]);
    b
}
#[test]
fn preflight_rejects_color_depth_subsampling_extents_and_truncations() {
    let c = Jpeg2000Codec {
        encoder: "absent".into(),
        decoder: "absent".into(),
        timeout: Duration::from_secs(1),
    };
    let b = codestream(2, 3);
    for n in 0..b.len() {
        assert!(c.decode(&b[..n]).is_err());
    }
    for (offset, value) in [
        (41, 3),
        (42, 15),
        (42, 135),
        (43, 2),
        (44, 2),
        (19, 1),
        (23, 1),
        (27, 3),
        (31, 4),
    ] {
        let mut bad = b.clone();
        bad[offset] = value;
        assert!(
            c.decode(&bad).unwrap_err().to_string().contains("JPEG2000"),
            "offset {offset}"
        );
    }
    assert!(c.decode(&codestream(5000, 5000)).is_err());
    let bad = CapturedImage::new(Image {
        width: 0,
        height: 1,
        pixels: vec![],
    });
    assert!(c.encode(&bad).is_err());
}
#[test]
fn decoder_preserves_only_explicit_metadata_and_checks_process_output() {
    let w = process::Workspace::new().unwrap();
    let executable = w.0.join(if cfg!(windows) { "codec.exe" } else { "codec" });
    assert!(
        Command::new("rustc")
            .args(["--edition=2024", "tests/support/jpeg2000_stub.rs", "-o"])
            .arg(&executable)
            .status()
            .unwrap()
            .success()
    );
    let c = Jpeg2000Codec {
        encoder: executable.clone(),
        decoder: executable,
        // Ordinary child startup is not the behavior under test here. Leave
        // headroom for OS executable checks and parallel compiler load.
        timeout: Duration::from_secs(5),
    };
    let b = codestream(2, 3);
    for mode in [
        "normal",
        "wrong_geometry",
        "wrong_depth",
        "short",
        "trailing",
        "failure",
    ] {
        fs::write(w.0.join("mode"), mode).unwrap();
        let result = c.decode(&b);
        assert_eq!(result.is_ok(), mode == "normal", "{mode}: {result:?}");
        if let Ok(image) = result {
            assert_eq!(image.image.pixels, [1, 2, 3, 4, 5, 6]);
            assert_eq!(image.metadata.resolution, Resolution::Unknown);
        }
    }
    fs::write(w.0.join("mode"), "hang").unwrap();
    let short_deadline = Jpeg2000Codec {
        timeout: Duration::from_millis(300),
        ..c.clone()
    };
    assert!(
        short_deadline
            .decode(&b)
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    fs::write(w.0.join("mode"), "normal").unwrap();
    let mut metadata = uareu::metadata::CaptureMetadata {
        resolution: Resolution::declared(400.0, 500.0).unwrap(),
        ..Default::default()
    };
    metadata.processing.push(ProcessingStep::Rotate180);
    let decoded = c.decode_with_metadata(&b, metadata.clone()).unwrap();
    assert_eq!(decoded.metadata.resolution, metadata.resolution);
    assert_eq!(
        decoded.metadata.processing,
        [
            ProcessingStep::Rotate180,
            ProcessingStep::Jpeg2000Decompression
        ]
    );
}

#[test]
fn record_deadline_and_encoding_mismatch_fail_without_mutation() {
    use uareu::image_record::{ImageCompression, ImageView, MultiImageRecord};
    let c = Jpeg2000Codec {
        encoder: "absent".into(),
        decoder: "absent".into(),
        timeout: Duration::from_secs(1),
    };
    let mut record = MultiImageRecord {
        capture_device_id: 0,
        acquisition_level: 0,
        scale_units: 1,
        scan_resolution_x: 500,
        scan_resolution_y: 500,
        image_resolution_x: 500,
        image_resolution_y: 500,
        compression: ImageCompression::Raw8Bit,
        images: vec![ImageView {
            finger_position: 1,
            view_count: 1,
            view_number: 1,
            quality: 80,
            impression: 0,
            width: 2,
            height: 3,
            data: vec![1, 2, 3, 4, 5, 6],
        }],
    };
    let before = serde_json::to_vec(&record).unwrap();
    assert!(record.compress_jpeg2000(&c, Duration::ZERO).is_err());
    assert!(
        record
            .decompress_jpeg2000(&c, Duration::from_secs(1))
            .is_err()
    );
    assert_eq!(before, serde_json::to_vec(&record).unwrap());
    record.compression = ImageCompression::OpaqueWsq;
    assert!(
        record
            .compress_jpeg2000(&c, Duration::from_secs(1))
            .is_err()
    );
    assert!(
        record
            .decompress_jpeg2000(&c, Duration::from_secs(1))
            .is_err()
    );
}
