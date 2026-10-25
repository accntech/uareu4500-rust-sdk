// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in actual OpenJPEG integration; not JPEG2000 certification.
#[path = "support/reference_process.rs"]
mod process;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};
use uareu::{
    frame::Image,
    image_record::{self, ImageCompression, ImageView, MultiImageRecord},
    jpeg2000::Jpeg2000Codec,
    metadata::{CapturedImage, Resolution},
};
#[test]
#[ignore = "requires UAREU_OPENJPEG_ENCODER and UAREU_OPENJPEG_DECODER executables"]
fn real_lossless_codestream_and_records_preserve_pixels_and_declarations() {
    let c = Jpeg2000Codec {
        encoder: PathBuf::from(
            std::env::var_os("UAREU_OPENJPEG_ENCODER").expect("set UAREU_OPENJPEG_ENCODER"),
        ),
        decoder: PathBuf::from(
            std::env::var_os("UAREU_OPENJPEG_DECODER").expect("set UAREU_OPENJPEG_DECODER"),
        ),
        timeout: Duration::from_secs(10),
    };
    let w = process::Workspace::new().unwrap();
    let image = CapturedImage::new(Image {
        width: 64,
        height: 64,
        pixels: (0..4096)
            .map(|i| ((i * 13 + i / 64 * 7) % 256) as u8)
            .collect(),
    });
    let b = c.encode(&image).unwrap();
    let d = c.decode(&b).unwrap();
    assert_eq!(d.image, image.image);
    assert_eq!(d.metadata.resolution, Resolution::Unknown);
    let input = w.0.join("direct.j2k");
    let output = w.0.join("direct.pgm");
    fs::write(&input, &b).unwrap();
    process::run(
        &c.decoder,
        &[
            "-i".into(),
            input.clone().into_os_string(),
            "-o".into(),
            output.clone().into_os_string(),
            "-threads".into(),
            "1".into(),
        ],
        &w.0,
        Duration::from_secs(10),
    )
    .unwrap();
    let direct = fs::read(&output).unwrap();
    assert_eq!(&direct[direct.len() - 4096..], image.image.pixels);
    for n in [0, 1, 2, 3, 10, 44, b.len() / 2, b.len() - 1] {
        assert!(c.decode(&b[..n]).is_err());
    }
    // OpenJPEG can encode RGB; our adapter must reject it rather than silently decode component0.
    let color = w.0.join("color.ppm");
    let mut rgb = b"P6\n2 2\n255\n".to_vec();
    rgb.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    fs::write(&color, rgb).unwrap();
    let compressed = w.0.join("color.j2k");
    process::run(
        &c.encoder,
        &[
            "-i".into(),
            color.into_os_string(),
            "-o".into(),
            compressed.clone().into_os_string(),
            "-n".into(),
            "1".into(),
            "-threads".into(),
            "1".into(),
        ],
        &w.0,
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(c.decode(&fs::read(compressed).unwrap()).is_err());
    let mut record = MultiImageRecord {
        capture_device_id: 1,
        acquisition_level: 0,
        scale_units: 1,
        scan_resolution_x: 400,
        scan_resolution_y: 500,
        image_resolution_x: 400,
        image_resolution_y: 500,
        compression: ImageCompression::Raw8Bit,
        images: vec![
            ImageView {
                finger_position: 1,
                view_count: 2,
                view_number: 1,
                quality: 80,
                impression: 0,
                width: 64,
                height: 64,
                data: image.image.pixels.clone(),
            },
            ImageView {
                finger_position: 1,
                view_count: 2,
                view_number: 2,
                quality: 70,
                impression: 0,
                width: 2,
                height: 2,
                data: vec![0, 255, 128, 64],
            },
        ],
    };
    for units in [1, 2] {
        record.scale_units = units;
        let original = image_record::encode_images(&record).unwrap();
        let encoded = record
            .compress_jpeg2000(&c, Duration::from_secs(10))
            .unwrap();
        assert_eq!(encoded.compression, ImageCompression::OpaqueJpeg2000);
        let bytes = image_record::encode_images(&encoded).unwrap();
        let imported = image_record::decode_images(&bytes).unwrap();
        let raw = imported
            .decompress_jpeg2000(&c, Duration::from_secs(10))
            .unwrap();
        assert_eq!(image_record::encode_images(&raw).unwrap(), original);
        assert_eq!(image_record::encode_images(&record).unwrap(), original);
        let mut conflict = imported.clone();
        conflict.images[1].width = 3;
        assert!(
            conflict
                .decompress_jpeg2000(&c, Duration::from_secs(10))
                .is_err()
        );
    }
    println!(
        "{}",
        serde_json::json!({"status":"passed","encoded_bytes":b.len(),"encoder_sha256":format!("{:x}",Sha256::digest(fs::read(&c.encoder).unwrap())),"decoder_sha256":format!("{:x}",Sha256::digest(fs::read(&c.decoder).unwrap())),"input_pixels_sha256":format!("{:x}",Sha256::digest(&image.image.pixels)),"limitation":"OpenJPEG integration only; lossless selected fixtures, not certification or reader scale"})
    );
}
