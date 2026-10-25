// SPDX-License-Identifier: GPL-3.0-or-later
use uareu4500::frame::{HEADER_SIZE, HEIGHT, Image, InvalidFrame, WIDTH, decode_frame};

const PLAIN: &[u8] = include_bytes!("../fixtures/synthetic-plain.wire");
const ENCODED: &[u8] = include_bytes!("../fixtures/synthetic-scrambled.wire");

fn key(index: u8) -> anyhow::Result<u32> {
    match index {
        254 => Ok(0x1367_9245),
        255 => Ok(0x89ab_cdef),
        0 => Ok(0xf00d_cafe),
        _ => anyhow::bail!("unexpected synthetic index {index}"),
    }
}

#[test]
fn plaintext_vector_matches_pre_encoding_sensor_canvas() {
    let image = decode_frame(PLAIN, true, |_| panic!("plaintext has no key exchange")).unwrap();
    assert_eq!((image.width, image.height), (WIDTH, HEIGHT));
    assert_eq!(
        image.pixels.as_slice(),
        include_bytes!("../fixtures/synthetic-plain.gray")
    );
}

#[test]
fn synthetic_vector_covers_rekeys_gaps_and_state_continuity() {
    let mut requests = Vec::new();
    let image = decode_frame(ENCODED, true, |index| {
        requests.push(index);
        key(index)
    })
    .unwrap();
    assert_eq!(requests, [254, 255, 0]);
    let expected = include_bytes!("../fixtures/synthetic-scrambled.gray");
    assert_eq!(image.pixels.len(), expected.len());
    let mismatch = image.pixels.iter().zip(expected).position(|(a, b)| a != b);
    assert_eq!(mismatch, None, "different pixel at {mismatch:?}");
}

#[test]
fn native_orientation_and_rotated_canvas_have_the_same_samples() {
    let native = decode_frame(PLAIN, false, |_| unreachable!()).unwrap();
    let rotated = decode_frame(PLAIN, true, |_| unreachable!()).unwrap();
    assert_eq!(
        native.pixels.iter().rev().copied().collect::<Vec<_>>(),
        rotated.pixels
    );
    assert_eq!(&native.pixels[..WIDTH], &[225; WIDTH]);
}

#[test]
fn malformed_layouts_fail_before_key_io() {
    let mut variants = vec![
        vec![],
        vec![0; HEADER_SIZE - 1],
        ENCODED[..ENCODED.len() - 1].to_vec(),
    ];
    for (offset, value) in [(4, 0), (5, 255), (17, 255), (17, 0)] {
        let mut raw = ENCODED.to_vec();
        raw[offset] = value;
        variants.push(raw);
    }
    for raw in variants {
        let error = decode_frame(&raw, false, |_| {
            panic!("invalid layout must not query a key")
        })
        .unwrap_err();
        assert!(error.downcast_ref::<InvalidFrame>().is_some(), "{error:#}");
    }
}

#[test]
fn full_canvas_row_capacity_is_accepted_without_overflow() {
    let mut raw = vec![0; HEADER_SIZE + WIDTH * HEIGHT];
    raw[4..6].copy_from_slice(&(HEIGHT as u16).to_le_bytes());
    raw[17] = 200;
    raw[19] = 90;
    let image = decode_frame(&raw, false, |_| unreachable!()).unwrap();
    assert_eq!(image.pixels, vec![255; WIDTH * HEIGHT]);
}

#[test]
fn a_key_transport_error_is_preserved_as_a_non_frame_error() {
    let error = decode_frame(ENCODED, false, |_| {
        anyhow::bail!("synthetic transport unavailable")
    })
    .unwrap_err();
    assert!(error.downcast_ref::<InvalidFrame>().is_none());
    assert!(format!("{error:#}").contains("synthetic transport unavailable"));
}

#[test]
fn arbitrary_device_bytes_do_not_panic() {
    let mut state = 0xdea1_beefu32;
    for length in [0, 1, 63, 64, 65, 384, 832, 1216, 3136, 111424, 111425] {
        for _ in 0..8 {
            let bytes: Vec<_> = (0..length)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    (state >> 24) as u8
                })
                .collect();
            if let Ok(image) = decode_frame(&bytes, true, |_| Ok(0)) {
                assert_eq!(image.pixels.len(), WIDTH * HEIGHT);
            }
        }
    }
}

#[test]
fn png_serialization_is_exclusive_private_and_lossless() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("uareu-png-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("sample.png");
    let image = Image {
        width: 3,
        height: 1,
        pixels: vec![1, 127, 254],
    };
    image.save_png(&path).unwrap();
    let original_file = std::fs::read(&path).unwrap();
    assert!(image.save_png(&path).is_err());
    assert_eq!(original_file, std::fs::read(&path).unwrap());
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!(&pixels[..info.buffer_size()], image.pixels);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let bad = Image {
        width: 0,
        height: 1,
        pixels: vec![],
    };
    let bad_path = directory.join("bad.png");
    assert!(bad.save_png(&bad_path).is_err());
    assert!(!bad_path.exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn scrambling_variance_threshold_is_inclusive() {
    let mut raw = vec![0; HEADER_SIZE + WIDTH * 2];
    raw[4..6].copy_from_slice(&2u16.to_le_bytes());
    raw[17] = 2;
    // Six symmetric +/-90 pairs and 186 +/-70 pairs: variance exactly 5000.
    for column in 0..WIDTH {
        let pair = column % (WIDTH / 2);
        let amplitude = if pair < 6 { 90 } else { 70 };
        let sum = if column < WIDTH / 2 {
            200 + amplitude
        } else {
            200 - amplitude
        };
        raw[HEADER_SIZE + column] = (sum / 2) as u8;
        raw[HEADER_SIZE + WIDTH + column] = (sum / 2) as u8;
    }
    let mut queries = 0;
    decode_frame(&raw, false, |_| {
        queries += 1;
        Ok(0)
    })
    .unwrap();
    assert_eq!(queries, 1);
}

#[test]
fn overlapping_descriptors_expand_the_completed_sensor_buffer() {
    let mut raw = vec![0; HEADER_SIZE + WIDTH * 2];
    raw[4..6].copy_from_slice(&2u16.to_le_bytes());
    raw[16..20].copy_from_slice(&[3, 1, 2, 2]);
    for (index, value) in raw[HEADER_SIZE..].iter_mut().enumerate() {
        *value = if index % 2 == 0 { 0 } else { 255 };
    }
    // With a zero stream state, both transforms shift samples left and append
    // zero. The repeated first row must reflect BOTH transforms, not a snapshot
    // taken after the first transform alone.
    let image = decode_frame(&raw, false, |_| Ok(0)).unwrap();
    assert_eq!(image.pixels[0], 255);
    assert_eq!(&image.pixels[..WIDTH], &image.pixels[WIDTH..WIDTH * 2]);
    assert_eq!(image.pixels[WIDTH * 2 - 2], 255);
}
