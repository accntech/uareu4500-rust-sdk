// SPDX-License-Identifier: GPL-3.0-or-later
//! Coordinate-grid characterization only; this does not establish physical DPI.
use uareu::{
    frame::{HEADER_SIZE, HEIGHT, WIDTH, decode_frame_with_metadata},
    metadata::{Region, Resolution},
};

#[test]
fn descriptor_grid_maps_reused_rows_padding_and_rotation_without_assigning_scale() {
    let mut wire = vec![0; HEADER_SIZE + WIDTH * 4];
    wire[4..6].copy_from_slice(&4u16.to_le_bytes());
    wire[16..22].copy_from_slice(&[1, 2, 0, 2, 0, 2]);
    for row in 0..4 {
        for x in 0..WIDTH {
            wire[HEADER_SIZE + row * WIDTH + x] = (20 + row * 11 + x % 17) as u8;
        }
    }
    let unrotated =
        decode_frame_with_metadata(&wire, false, |_| panic!("plaintext must not query a key"))
            .unwrap();
    let rotated = decode_frame_with_metadata(&wire, true, |_| unreachable!()).unwrap();
    for (outrow, source) in [0, 1, 0, 1, 2, 3].into_iter().enumerate() {
        for x in 0..WIDTH {
            assert_eq!(
                unrotated.image.pixels[outrow * WIDTH + x],
                255 - wire[HEADER_SIZE + source * WIDTH + x]
            );
        }
    }
    assert!(
        unrotated.image.pixels[6 * WIDTH..]
            .iter()
            .all(|&p| p == 255)
    );
    assert_eq!(
        rotated.image.pixels,
        unrotated
            .image
            .pixels
            .iter()
            .rev()
            .copied()
            .collect::<Vec<_>>()
    );
    assert_eq!(
        unrotated.metadata.valid_region,
        Some(Region {
            x: 0,
            y: 0,
            width: WIDTH,
            height: 6
        })
    );
    assert_eq!(
        rotated.metadata.valid_region,
        Some(Region {
            x: 0,
            y: HEIGHT - 6,
            width: WIDTH,
            height: 6
        })
    );
    assert!(matches!(unrotated.metadata.resolution, Resolution::Unknown));
    assert!(matches!(rotated.metadata.resolution, Resolution::Unknown));
}
