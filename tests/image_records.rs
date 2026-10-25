use uareu::image_record::{self, ImageCompression, ImageView, MultiImageRecord};

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
        images: vec![view(2, 2, 1), view(7, 1, 1), view(2, 2, 2)],
    }
}
fn view(finger_position: u8, view_count: u8, view_number: u8) -> ImageView {
    ImageView {
        finger_position,
        view_count,
        view_number,
        quality: 80,
        impression: 0,
        width: 2,
        height: 2,
        data: vec![1, 2, 3, 4],
    }
}
#[test]
fn raw_multiple_views_preserve_payloads_and_legacy_decoder_rejects_them() {
    let data = record();
    let bytes = image_record::encode_images(&data).unwrap();
    assert_eq!(bytes.len(), 86);
    assert_eq!(
        &bytes[32..50],
        &[0, 0, 0, 18, 2, 2, 1, 80, 0, 0, 2, 0, 2, 0, 1, 2, 3, 4]
    );
    let decoded = image_record::decode_images(&bytes).unwrap();
    assert_eq!(decoded.images.len(), 3);
    assert_eq!(decoded.images[2].view_number, 2);
    assert_eq!(image_record::encode_images(&decoded).unwrap(), bytes);
    assert_eq!(decoded.raw_image(2).unwrap().image.pixels, vec![1, 2, 3, 4]);
    assert!(image_record::decode(&bytes).is_err());
    for end in 0..bytes.len() {
        assert!(image_record::decode_images(&bytes[..end]).is_err());
    }
}
#[test]
fn rejects_bad_fields_lengths_counts_and_atomic_edits() {
    let mut data = record();
    data.edit_image(2, |v| v.quality = 50).unwrap();
    assert_eq!(data.images[2].quality, 50);
    assert!(data.edit_image(2, |v| v.view_number = 1).is_err());
    assert_eq!(data.images[2].view_number, 2);
    assert!(data.edit(|r| r.images[0].data.clear()).is_err());
    assert_eq!(data.images[0].data.len(), 4);
    assert!(data.edit_image(3, |_| {}).is_err());
    let bytes = image_record::encode_images(&data).unwrap();
    for (offset, value) in [
        (8, 1),
        (14, 0xf0),
        (18, 2),
        (19, 0),
        (28, 7),
        (29, 1),
        (30, 1),
        (35, 13),
        (36, 11),
        (37, 3),
        (38, 0),
        (39, 101),
        (40, 4),
        (42, 0),
        (45, 1),
    ] {
        let mut invalid = bytes.clone();
        invalid[offset] = value;
        assert!(
            image_record::decode_images(&invalid).is_err(),
            "offset {offset}"
        );
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert!(image_record::decode_images(&trailing).is_err());
}
#[test]
fn independent_single_image_envelope_and_legacy_compatibility() {
    let bytes = [
        b'F', b'I', b'R', 0, b'0', b'1', b'0', 0, 0, 0, 0, 0, 0, 47, 0, 0, 0, 0, 1, 1, 1, 244, 1,
        244, 1, 244, 1, 244, 8, 0, 0, 0, 0, 0, 0, 15, 2, 1, 1, 80, 0, 0, 1, 0, 1, 0, 42,
    ];
    let data = image_record::decode_images(&bytes).unwrap();
    assert_eq!(data.images[0].data, vec![42]);
    assert_eq!(image_record::encode_images(&data).unwrap(), bytes);
    let old = image_record::decode(&bytes).unwrap();
    assert_eq!(
        image_record::encode(&old.image, &old.options).unwrap(),
        bytes
    );
    assert_eq!(data.raw_image(0).unwrap().image, old.image.image);
}

#[test]
fn compressed_payload_transport_is_exact_and_never_fabricates_raw_pixels() {
    // Transport fixtures deliberately include no valid codec stream. This API
    // preserves opaque bytes; codecs must validate them before image use.
    for (compression, code) in [
        (ImageCompression::OpaqueWsq, 2),
        (ImageCompression::OpaqueJpeg2000, 4),
    ] {
        let mut data = record();
        data.compression = compression;
        data.images[0].data = vec![0x11, 0x22, 0x33];
        let bytes = image_record::encode_images(&data).unwrap();
        assert_eq!(bytes[29], code);
        let decoded = image_record::decode_images(&bytes).unwrap();
        assert_eq!(decoded.compression, compression);
        assert_eq!(decoded.images[0].data, vec![0x11, 0x22, 0x33]);
        assert!(decoded.raw_image(0).is_err());
        assert_eq!(image_record::encode_images(&decoded).unwrap(), bytes);
        assert!(image_record::decode(&bytes).is_err());
        assert!(data.edit_image(0, |v| v.data.clear()).is_err());
        assert!(
            data.edit_image(0, |v| v.data.resize(4 * 1024 * 1024 + 1, 0))
                .is_err()
        );
    }
}

#[test]
fn image_limits_and_declared_scale_are_checked() {
    let mut data = record();
    assert!(data.edit(|r| r.images.clear()).is_err());
    assert!(data.edit(|r| r.images.resize(256, view(0, 1, 1))).is_err());
    assert!(
        data.edit_image(0, |v| {
            v.width = 1025;
            v.height = 1024;
            v.data.resize(1025 * 1024, 1);
        })
        .is_err()
    );
    assert!(data.edit(|r| r.image_resolution_x = 501).is_err());
    data.edit(|r| {
        r.scale_units = 2;
        r.scan_resolution_x = 197;
        r.scan_resolution_y = 197;
        r.image_resolution_x = 197;
        r.image_resolution_y = 197;
    })
    .unwrap();
    let raw = data.raw_image(0).unwrap();
    match raw.metadata.resolution {
        uareu::metadata::Resolution::Known { x_ppi, y_ppi, .. } => {
            assert_eq!(x_ppi, 197.0 * 2.54);
            assert_eq!(y_ppi, 197.0 * 2.54);
        }
        _ => panic!("header scale was not preserved"),
    }
}

#[test]
fn wsq_record_pipeline_validates_metadata_and_uses_one_deadline() {
    use std::{
        fs,
        process::Command,
        time::{Duration, Instant},
    };
    use uareu::wsq::WsqCodec;
    #[allow(dead_code)]
    #[path = "support/reference_process.rs"]
    mod process;
    let workspace = process::Workspace::new().unwrap();
    let source = workspace.0.join("stub.rs");
    let executable = workspace
        .0
        .join(if cfg!(windows) { "stub.exe" } else { "stub" });
    let stub = fs::read_to_string("tests/support/wsq_stub.rs")
        .unwrap()
        .replace(
            "let input = PathBuf::from",
            "std::thread::sleep(Duration::from_millis(150));\n    let input = PathBuf::from",
        );
    fs::write(&source, stub).unwrap();
    assert!(
        Command::new("rustc")
            .args(["--edition=2024"])
            .arg(&source)
            .arg("-o")
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
    let mut data = record();
    data.images = vec![view(2, 2, 1), view(2, 2, 2)];
    for image in &mut data.images {
        image.width = 256;
        image.height = 256;
        image.data = vec![128; 65536];
    }
    fs::write(workspace.0.join("mode"), "encode").unwrap();
    let compressed = data
        .compress_wsq(&codec, 0.75, Duration::from_secs(2))
        .unwrap();
    assert_eq!(compressed.compression, ImageCompression::OpaqueWsq);
    assert_eq!(data.compression, ImageCompression::Raw8Bit);
    assert_eq!(compressed.images[1].view_number, 2);
    assert!(
        compressed
            .compress_wsq(&codec, 0.75, Duration::from_secs(2))
            .is_err()
    );
    assert!(
        data.compress_wsq(&codec, f64::NAN, Duration::from_secs(2))
            .is_err()
    );
    for deadline in [Duration::ZERO, Duration::from_secs(301)] {
        assert!(data.compress_wsq(&codec, 0.75, deadline).is_err());
        assert!(compressed.decompress_wsq(&codec, deadline).is_err());
    }
    for timeout in [Duration::ZERO, Duration::from_secs(301)] {
        let mut invalid_codec = codec.clone();
        invalid_codec.timeout = timeout;
        assert!(
            data.compress_wsq(&invalid_codec, 0.75, Duration::from_secs(2))
                .is_err()
        );
        assert!(
            compressed
                .decompress_wsq(&invalid_codec, Duration::from_secs(2))
                .is_err()
        );
    }
    fs::write(workspace.0.join("mode"), "normal").unwrap();
    assert!(
        compressed
            .decompress_wsq(&codec, Duration::from_secs(2))
            .is_err()
    ); // dimensions conflict
    let mut small = compressed.clone();
    for image in &mut small.images {
        image.width = 2;
        image.height = 3;
    }
    let raw = small
        .decompress_wsq(&codec, Duration::from_secs(2))
        .unwrap();
    assert_eq!(raw.compression, ImageCompression::Raw8Bit);
    assert_eq!(raw.images[1].data, [1, 2, 3, 4, 5, 6]);
    let mut wrong_ppi = small.clone();
    wrong_ppi.image_resolution_x = 250;
    assert!(
        wrong_ppi
            .decompress_wsq(&codec, Duration::from_secs(2))
            .is_err()
    );
    fs::write(workspace.0.join("mode"), "unknown_ppi").unwrap();
    assert!(
        wrong_ppi
            .decompress_wsq(&codec, Duration::from_secs(2))
            .is_ok()
    );
    let start = Instant::now();
    assert!(
        small
            .decompress_wsq(&codec, Duration::from_millis(220))
            .is_err()
    );
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(small.compression, ImageCompression::OpaqueWsq);
    assert_eq!(small.images[0].data, compressed.images[0].data);
    let mut jpeg = small;
    jpeg.compression = ImageCompression::OpaqueJpeg2000;
    assert!(jpeg.decompress_wsq(&codec, Duration::from_secs(2)).is_err());
}

#[test]
#[ignore = "requires UAREU_CWSQ_EXE and UAREU_DWSQ_EXE; executes real NBIS"]
fn real_nbis_multi_image_record_compression() {
    use std::{path::PathBuf, time::Duration};
    use uareu::wsq::WsqCodec;
    let codec = WsqCodec {
        encoder: PathBuf::from(std::env::var_os("UAREU_CWSQ_EXE").expect("set UAREU_CWSQ_EXE")),
        decoder: PathBuf::from(std::env::var_os("UAREU_DWSQ_EXE").expect("set UAREU_DWSQ_EXE")),
        timeout: Duration::from_secs(10),
    };
    let mut data = record();
    data.images = vec![view(2, 2, 1), view(2, 2, 2)];
    for (index, image) in data.images.iter_mut().enumerate() {
        image.width = 256;
        image.height = 256;
        image.data = (0..65536)
            .map(|i| (40 + ((i % 256 / 4 + i / 256 / 9 + index) % 12) * 15) as u8)
            .collect();
    }
    for units in [1, 2] {
        data.scale_units = units;
        let density = if units == 1 { 500 } else { 197 };
        data.image_resolution_x = density;
        data.image_resolution_y = density;
        data.scan_resolution_x = density;
        data.scan_resolution_y = density;
        let original = image_record::encode_images(&data).unwrap();
        let compressed = data
            .compress_wsq(&codec, 0.75, Duration::from_secs(20))
            .unwrap();
        assert_eq!(compressed.images[1].view_number, 2);
        assert_eq!(compressed.scale_units, units);
        let wire = image_record::encode_images(&compressed).unwrap();
        let parsed = image_record::decode_images(&wire).unwrap();
        let raw = parsed
            .decompress_wsq(&codec, Duration::from_secs(20))
            .unwrap();
        assert_eq!(raw.image_resolution_x, density);
        assert_eq!(raw.images.len(), 2);
        for (decoded, encoded) in raw.images.iter().zip(&compressed.images) {
            let direct = codec.decode(&encoded.data).unwrap();
            assert_eq!(decoded.data, direct.image.pixels);
            assert_eq!((decoded.width, decoded.height), (256, 256));
            assert_eq!(decoded.quality, encoded.quality);
        }
        assert_eq!(image_record::encode_images(&data).unwrap(), original);
        println!(
            "units={units}, views={}, compressed_record_bytes={}",
            raw.images.len(),
            wire.len()
        );
    }
}
