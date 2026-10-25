// SPDX-License-Identifier: GPL-3.0-or-later
use uareu::{
    frame::Image,
    metadata::{CapturedImage, ProcessingStep, Region, Resolution},
    processing::{FlatFieldConfig, FlatFieldProfile, FlatFieldReference},
};
fn image(pixels: &[u8]) -> CapturedImage {
    CapturedImage::new(Image {
        width: pixels.len(),
        height: 1,
        pixels: pixels.to_vec(),
    })
}
fn profile(dark: &CapturedImage, bright: &CapturedImage) -> FlatFieldProfile {
    FlatFieldProfile::from_references(
        &[FlatFieldReference {
            label: "declared dark",
            image: dark,
        }],
        &[FlatFieldReference {
            label: "declared bright",
            image: bright,
        }],
        FlatFieldConfig::default(),
    )
    .unwrap()
}
#[test]
fn numeric_correction_clamps_and_preserves_input_metadata() {
    let dark = image(&[10, 20, 10, 20]);
    let bright = image(&[110, 220, 110, 220]);
    let p = profile(&dark, &bright);
    let mut input = image(&[60, 120, 0, 255]);
    input.metadata.resolution = Resolution::declared(400.0, 500.0).unwrap();
    input.metadata.valid_region = Some(Region {
        x: 1,
        y: 0,
        width: 2,
        height: 1,
    });
    input.metadata.processing.push(ProcessingStep::Rotate180);
    let original = input.clone();
    let serialized = serde_json::to_vec(&p).unwrap();
    let restored: FlatFieldProfile = serde_json::from_slice(&serialized).unwrap();
    assert_eq!(restored.apply(&input).unwrap(), p.apply(&input).unwrap());
    let output = p.apply(&input).unwrap();
    assert_eq!(output.image.pixels, [128, 128, 0, 255]);
    assert_eq!(input, original);
    assert_eq!(output.metadata.resolution, input.metadata.resolution);
    assert_eq!(output.metadata.valid_region, input.metadata.valid_region);
    assert_eq!(
        output.metadata.processing,
        vec![
            ProcessingStep::Rotate180,
            ProcessingStep::FlatFieldCorrection {
                profile_sha256: p.sha256()
            }
        ]
    );
    assert_eq!(p.sha256(), profile(&dark, &bright).sha256());
    let mut changed_metadata = dark.clone();
    changed_metadata.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
    assert_ne!(p.sha256(), profile(&changed_metadata, &bright).sha256());
    assert_ne!(
        p.sha256(),
        profile(&image(&[11, 20, 10, 20]), &bright).sha256()
    );
}
#[test]
fn identity_and_averaged_references_are_explicit() {
    let dark = image(&[0, 0]);
    let bright = image(&[255, 255]);
    let input = image(&[17, 240]);
    assert_eq!(
        profile(&dark, &bright).apply(&input).unwrap().image,
        input.image
    );
    assert_eq!(
        profile(&dark, &bright)
            .apply(&input)
            .unwrap()
            .metadata
            .resolution,
        Resolution::Unknown
    );
    let d1 = image(&[0]);
    let d2 = image(&[20]);
    let b = image(&[110]);
    let p = FlatFieldProfile::from_references(
        &[
            FlatFieldReference {
                label: "d1",
                image: &d1,
            },
            FlatFieldReference {
                label: "d2",
                image: &d2,
            },
        ],
        &[FlatFieldReference {
            label: "b",
            image: &b,
        }],
        FlatFieldConfig::default(),
    )
    .unwrap();
    assert_eq!(p.apply(&image(&[60])).unwrap().image.pixels, [128]);
}

#[test]
fn serialized_noninteger_gain_preserves_profile_integrity() {
    let p = profile(&image(&[0]), &image(&[19]));
    let serialized = serde_json::to_string(&p).unwrap();
    let restored: FlatFieldProfile = serde_json::from_str(&serialized).unwrap();
    restored.validate().unwrap();
    assert_eq!(restored.sha256(), p.sha256());
    let input = image(&[10]);
    assert_eq!(restored.apply(&input).unwrap(), p.apply(&input).unwrap());
}

#[test]
fn repeated_references_at_minimum_response_are_accepted() {
    let dark = image(&[0]);
    let bright = image(&[8]);
    let darks = [FlatFieldReference {
        label: "dark",
        image: &dark,
    }; 6];
    let brights = [FlatFieldReference {
        label: "bright",
        image: &bright,
    }; 6];
    let p =
        FlatFieldProfile::from_references(&darks, &brights, FlatFieldConfig::default()).unwrap();
    assert_eq!(p.apply(&image(&[4])).unwrap().image.pixels, [128]);
}

#[test]
fn fractional_reference_means_at_minimum_response_are_accepted() {
    let dark = [image(&[0]), image(&[1]), image(&[1])];
    let bright = [image(&[8]), image(&[9]), image(&[9])];
    let darks = dark.each_ref().map(|image| FlatFieldReference {
        label: "dark",
        image,
    });
    let brights = bright.each_ref().map(|image| FlatFieldReference {
        label: "bright",
        image,
    });
    let p =
        FlatFieldProfile::from_references(&darks, &brights, FlatFieldConfig::default()).unwrap();
    p.validate().unwrap();
}
#[test]
fn rejects_bad_references_configuration_and_tampered_profiles() {
    let d = image(&[10]);
    let b = image(&[10]);
    let dr = [FlatFieldReference {
        label: "dark",
        image: &d,
    }];
    let br = [FlatFieldReference {
        label: "bright",
        image: &b,
    }];
    assert!(FlatFieldProfile::from_references(&dr, &br, FlatFieldConfig::default()).is_err());
    assert!(FlatFieldProfile::from_references(&[], &br, FlatFieldConfig::default()).is_err());
    assert!(
        FlatFieldProfile::from_references(
            &dr,
            &br,
            FlatFieldConfig {
                minimum_response: f64::NAN,
                ..FlatFieldConfig::default()
            }
        )
        .is_err()
    );
    let p = profile(&image(&[0]), &image(&[255]));
    assert!(p.apply(&image(&[10, 20])).is_err());
    let mut json = serde_json::to_value(&p).unwrap();
    json["gains"][0] = serde_json::json!(0.0);
    let invalid: FlatFieldProfile = serde_json::from_value(json).unwrap();
    assert!(invalid.apply(&image(&[100])).is_err());
}

#[test]
fn rejects_invalid_shapes_responses_and_hash_changes_before_output() {
    let dark = image(&[10, 10]);
    for bright in [image(&[9, 100]), image(&[11, 100]), image(&[100])] {
        assert!(
            FlatFieldProfile::from_references(
                &[FlatFieldReference {
                    label: "dark",
                    image: &dark
                }],
                &[FlatFieldReference {
                    label: "bright",
                    image: &bright
                }],
                FlatFieldConfig::default()
            )
            .is_err()
        );
    }
    let bright = image(&[100, 100]);
    assert!(
        FlatFieldProfile::from_references(
            &[FlatFieldReference {
                label: " ",
                image: &dark
            }],
            &[FlatFieldReference {
                label: "bright",
                image: &bright
            }],
            FlatFieldConfig::default()
        )
        .is_err()
    );
    let p = profile(&dark, &bright);
    for (field, value) in [
        ("gains", serde_json::json!([1.0])),
        ("offsets", serde_json::json!([-1.0, 10.0])),
        ("width", serde_json::json!(usize::MAX)),
        ("dark_sources", serde_json::json!([])),
        ("sha256", serde_json::json!(vec![0; 32])),
    ] {
        let mut json = serde_json::to_value(&p).unwrap();
        json[field] = value;
        let invalid: FlatFieldProfile = serde_json::from_value(json).unwrap();
        let input = image(&[50, 50]);
        let original = input.clone();
        assert!(invalid.apply(&input).is_err(), "{field}");
        assert_eq!(input, original);
    }
    let invalid = CapturedImage::new(Image {
        width: usize::MAX,
        height: 2,
        pixels: vec![],
    });
    assert!(p.apply(&invalid).is_err());
    let oversized = CapturedImage::new(Image {
        width: 16 * 1024 * 1024 + 1,
        height: 1,
        pixels: vec![],
    });
    assert!(
        FlatFieldProfile::from_references(
            &[FlatFieldReference {
                label: "too large",
                image: &oversized
            }],
            &[FlatFieldReference {
                label: "bright",
                image: &bright
            }],
            FlatFieldConfig::default()
        )
        .is_err()
    );
}
