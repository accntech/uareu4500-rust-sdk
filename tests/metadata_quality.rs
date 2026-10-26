// SPDX-License-Identifier: GPL-3.0-or-later
use std::{fs, path::PathBuf};
use uareu::{
    frame::{HEADER_SIZE, HEIGHT, Image, WIDTH, decode_frame, decode_frame_with_metadata},
    metadata::{
        CaptureMetadata, CapturedImage, ProcessingStep, Region, Resolution, ResolutionProvenance,
    },
    quality::{Nfiq2, QualityOutcome, assess, diagnostics},
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).unwrap();
        let path = std::env::temp_dir().join(format!(
            "uareu-metadata-test-{:032x}",
            u128::from_le_bytes(nonce)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn detailed_decode_preserves_legacy_pixels_and_tracks_coverage_and_rotation() {
    let mut wire = vec![0; HEADER_SIZE + 2 * WIDTH];
    wire[4] = 2;
    wire[16..20].copy_from_slice(&[1, 1, 0, 2]);
    wire[HEADER_SIZE] = 42;
    for rotate in [false, true] {
        let captured = decode_frame_with_metadata(&wire, rotate, |_| unreachable!()).unwrap();
        assert_eq!(
            captured.image,
            decode_frame(&wire, rotate, |_| unreachable!()).unwrap()
        );
        assert_eq!(captured.metadata.resolution, Resolution::Unknown);
        assert_eq!(
            captured.metadata.valid_region,
            Some(Region {
                x: 0,
                y: if rotate { HEIGHT - 3 } else { 0 },
                width: WIDTH,
                height: 3,
            })
        );
        assert!(
            captured
                .metadata
                .processing
                .contains(&ProcessingStep::Rotate180)
                == rotate
        );
        assert_eq!(diagnostics(&captured).unwrap().analyzed_pixels, WIDTH * 3);
    }
}

#[test]
fn png_round_trips_provenance_without_altering_pixels_or_overwriting() {
    let dir = Directory::new();
    let image = Image {
        width: 2,
        height: 1,
        pixels: vec![0, 255],
    };
    for (index, resolution) in [
        Resolution::Unknown,
        Resolution::declared(500.0, 500.0).unwrap(),
        Resolution::Known {
            x_ppi: 512.0,
            y_ppi: 500.0,
            provenance: ResolutionProvenance::DocumentedProfile {
                evidence: "測定値ではない: document example".into(),
            },
        },
    ]
    .into_iter()
    .enumerate()
    {
        let captured = CapturedImage {
            image: image.clone(),
            metadata: CaptureMetadata {
                resolution: resolution.clone(),
                ..Default::default()
            },
        };
        let path = dir.0.join(format!("{index}.png"));
        captured.save_png(&path).unwrap();
        assert_eq!(CapturedImage::load_png(&path).unwrap(), captured);
        let reader = png::Decoder::new(std::io::BufReader::new(fs::File::open(&path).unwrap()))
            .read_info()
            .unwrap();
        if let Resolution::Known { x_ppi, y_ppi, .. } = resolution {
            let physical = reader.info().pixel_dims.unwrap();
            assert_eq!(physical.unit, png::Unit::Meter);
            assert!((f64::from(physical.xppu) * 0.0254 - x_ppi).abs() <= 0.0127);
            assert!((f64::from(physical.yppu) * 0.0254 - y_ppi).abs() <= 0.0127);
        } else {
            assert!(reader.info().pixel_dims.is_none());
        }
        let before = fs::read(&path).unwrap();
        assert!(captured.save_png(&path).is_err());
        assert_eq!(before, fs::read(&path).unwrap());
    }
    let path = dir.0.join("legacy.png");
    image.save_png(&path).unwrap();
    assert_eq!(
        CapturedImage::load_png(path).unwrap().metadata,
        CaptureMetadata::default()
    );
}

#[test]
fn external_png_resolution_is_only_an_embedded_declaration() {
    let dir = Directory::new();
    let path = dir.0.join("external.png");
    let mut encoder = png::Encoder::new(fs::File::create(&path).unwrap(), 2, 1);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_pixel_dims(Some(png::PixelDimensions {
        xppu: 19685,
        yppu: 19685,
        unit: png::Unit::Meter,
    }));
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0, 255])
        .unwrap();
    let captured = CapturedImage::load_png(path).unwrap();
    assert!(matches!(
        captured.metadata.resolution,
        Resolution::Known {
            provenance: ResolutionProvenance::PngPhysicalDimensions,
            ..
        }
    ));
    assert_eq!(captured.metadata.valid_region, None);
    assert!(captured.metadata.processing.is_empty());
}

#[test]
fn diagnostics_exclude_only_declared_padding_and_have_no_acceptance_score() {
    let mut captured = CapturedImage::new(Image {
        width: 3,
        height: 2,
        pixels: vec![255, 0, 255, 255, 255, 255],
    });
    captured.metadata.valid_region = Some(Region {
        x: 1,
        y: 0,
        width: 1,
        height: 2,
    });
    let stats = diagnostics(&captured).unwrap();
    assert_eq!(
        (stats.minimum, stats.maximum, stats.analyzed_pixels),
        (0, 255, 2)
    );
    assert_eq!(stats.mean, 127.5);
    assert_eq!(stats.standard_deviation, 127.5);
    assert_eq!(stats.zero_fraction, 0.5);
    assert_eq!(stats.full_scale_fraction, 0.5);
    assert!((stats.known_padding_fraction.unwrap() - 2.0 / 3.0).abs() < 1e-12);
    assert!(matches!(
        assess(&captured, None).unwrap().quality,
        QualityOutcome::NotEvaluated
    ));
    captured.metadata.valid_region = None;
    assert_eq!(diagnostics(&captured).unwrap().known_padding_fraction, None);
    captured.metadata.valid_region = Some(Region {
        x: usize::MAX,
        y: 0,
        width: 1,
        height: 1,
    });
    assert!(diagnostics(&captured).is_err());
    for ppi in [0.0, -1.0, f64::NAN, f64::INFINITY, 100_001.0] {
        assert!(Resolution::declared(ppi, 500.0).is_err());
    }
}

#[test]
fn nfiq_rejects_unknown_or_non500_resolution_before_starting_an_engine() {
    let mut captured = CapturedImage::new(Image {
        width: 384,
        height: 290,
        pixels: vec![128; 384 * 290],
    });
    let engine = Nfiq2 {
        executable: "absent-engine".into(),
        model_info: None,
        timeout: std::time::Duration::from_secs(1),
    };
    assert!(matches!(
        engine.assess(&captured),
        QualityOutcome::UnsupportedInput { .. }
    ));
    captured.metadata.resolution = Resolution::declared(512.0, 512.0).unwrap();
    assert!(matches!(
        engine.assess(&captured),
        QualityOutcome::UnsupportedInput { .. }
    ));
    captured.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
    assert!(matches!(
        engine.assess(&captured),
        QualityOutcome::EngineUnavailable { .. }
    ));
}

#[test]
fn renamed_png_key_preserves_legacy_provenance_and_rejects_ambiguous_metadata() {
    let dir = Directory::new();
    let metadata = CaptureMetadata {
        resolution: Resolution::Unknown,
        valid_region: Some(Region {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }),
        processing: vec![ProcessingStep::IntensityInversion],
    };
    for (index, keys) in [
        vec!["uareu4500.metadata.v1"],
        vec!["uareu.metadata.v1"],
        vec!["uareu4500.metadata.v1", "uareu.metadata.v1"],
    ]
    .iter()
    .enumerate()
    {
        let path = dir.0.join(format!("metadata-key-{index}.png"));
        let mut encoder = png::Encoder::new(fs::File::create(&path).unwrap(), 1, 1);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        for key in keys {
            encoder
                .add_itxt_chunk((*key).into(), serde_json::to_string(&metadata).unwrap())
                .unwrap();
        }
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[42])
            .unwrap();
        if keys.len() > 1 {
            assert!(
                CapturedImage::load_png(path)
                    .unwrap_err()
                    .to_string()
                    .contains("duplicate image metadata")
            );
            continue;
        }
        let loaded = CapturedImage::load_png(path).unwrap();
        assert_eq!(loaded.metadata, metadata);
        assert_eq!(loaded.image.pixels, [42]);
        let output = dir.0.join(format!("rewritten-{index}.png"));
        loaded.save_png(&output).unwrap();
        let reader = png::Decoder::new(std::io::BufReader::new(fs::File::open(output).unwrap()))
            .read_info()
            .unwrap();
        let keywords: Vec<_> = reader
            .info()
            .utf8_text
            .iter()
            .map(|entry| entry.keyword.as_str())
            .collect();
        assert_eq!(keywords, ["uareu.metadata.v1"]);
    }
}

#[test]
fn png_rejects_metadata_conflicts_and_non_grayscale_input() {
    let dir = Directory::new();
    for color in [png::ColorType::Grayscale, png::ColorType::Rgb] {
        let path = dir.0.join(format!("{color:?}.png"));
        let mut encoder = png::Encoder::new(fs::File::create(&path).unwrap(), 1, 1);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        if color == png::ColorType::Grayscale {
            encoder.set_pixel_dims(Some(png::PixelDimensions {
                xppu: 19685,
                yppu: 19685,
                unit: png::Unit::Meter,
            }));
            let metadata = CaptureMetadata {
                resolution: Resolution::declared(512.0, 512.0).unwrap(),
                ..Default::default()
            };
            encoder
                .add_itxt_chunk(
                    "uareu.metadata.v1".into(),
                    serde_json::to_string(&metadata).unwrap(),
                )
                .unwrap();
        }
        encoder
            .write_header()
            .unwrap()
            .write_image_data(if color == png::ColorType::Grayscale {
                &[0]
            } else {
                &[0, 0, 0]
            })
            .unwrap();
        assert!(CapturedImage::load_png(path).is_err());
    }
}

#[test]
#[cfg(feature = "cli")]
fn quality_cli_reports_unknown_and_declared_resolution_without_touching_input() {
    let dir = Directory::new();
    let path = dir.0.join("input.png");
    CapturedImage::new(Image {
        width: 384,
        height: 290,
        pixels: vec![128; 384 * 290],
    })
    .save_png(&path)
    .unwrap();
    let original = fs::read(&path).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_uareu"))
            .args(["quality", "--input"])
            .arg(&path)
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["metadata"]["resolution"]["status"], "unknown");
    assert_eq!(report["quality"]["status"], "not_evaluated");
    let output = run(&["--ppi", "500", "--nfiq2", "missing-engine"]);
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["metadata"]["resolution"]["provenance"]["source"],
        "caller_declared"
    );
    assert_eq!(report["quality"]["status"], "engine_unavailable");
    let output = run(&["--nfiq2", "missing-engine"]);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["quality"]["status"], "unsupported_input");
    assert!(!run(&["--ppi", "NaN"]).status.success());
    assert_eq!(fs::read(path).unwrap(), original);
}
