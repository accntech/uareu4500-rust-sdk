// SPDX-License-Identifier: GPL-3.0-or-later
use std::time::Duration;
use uareu::{
    image_record::{ImageCompression, ImageView, MultiImageRecord},
    quality::{Nfiq2, QualityOutcome},
};
fn record() -> MultiImageRecord {
    MultiImageRecord {
        capture_device_id: 0,
        acquisition_level: 0,
        scale_units: 1,
        scan_resolution_x: 500,
        scan_resolution_y: 500,
        image_resolution_x: 500,
        image_resolution_y: 500,
        compression: ImageCompression::Raw8Bit,
        images: vec![
            ImageView {
                finger_position: 1,
                view_count: 2,
                view_number: 1,
                quality: 0,
                impression: 0,
                width: 10,
                height: 10,
                data: vec![100; 100],
            },
            ImageView {
                finger_position: 1,
                view_count: 2,
                view_number: 2,
                quality: 0,
                impression: 0,
                width: 200,
                height: 200,
                data: vec![100; 40000],
            },
        ],
    }
}
#[test]
fn record_quality_uses_explicit_view_and_never_decodes_opaque_payloads_as_raw() {
    let engine = Nfiq2 {
        executable: "missing-nfiq2".into(),
        model_info: None,
        timeout: Duration::from_secs(1),
    };
    let mut input = record();
    assert!(matches!(
        engine.assess_record_view(&input, 0),
        QualityOutcome::UnsupportedInput { .. }
    ));
    assert!(matches!(
        engine.assess_record_view(&input, 1),
        QualityOutcome::EngineUnavailable { .. }
    ));
    assert!(matches!(
        engine.assess_record_view(&input, 2),
        QualityOutcome::UnsupportedInput { .. }
    ));
    input.compression = ImageCompression::OpaqueWsq;
    assert!(matches!(
        engine.assess_record_view(&input, 1),
        QualityOutcome::UnsupportedInput { .. }
    ));
}

#[test]
fn ppcm_header_rounding_is_not_a_false_non500_rejection() {
    let engine = Nfiq2 {
        executable: "missing-nfiq2".into(),
        model_info: None,
        timeout: Duration::from_secs(1),
    };
    let mut input = record();
    input.scale_units = 2;
    input.scan_resolution_x = 197;
    input.scan_resolution_y = 197;
    input.image_resolution_x = 197;
    input.image_resolution_y = 197;
    assert!(matches!(
        engine.assess_record_view(&input, 1),
        QualityOutcome::EngineUnavailable { .. }
    ));
    assert!(
        matches!(input.raw_image(1).unwrap().metadata.resolution,uareu::metadata::Resolution::Known{x_ppi,..} if (x_ppi-500.38).abs()<1e-8)
    );
}
