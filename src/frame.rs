// SPDX-License-Identifier: GPL-3.0-or-later
// Protocol reference: libfprint; Copyright (C) 2007-2008 Daniel Drake,
// Copyright (C) 2012 Timo Teräs. See NOTICE for license origin and scope.
// Rust implementation and modifications: 2026-10-25, UareU Rust SDK contributors.
use std::{fmt, path::Path};
pub const WIDTH: usize = 384;
pub const HEIGHT: usize = 290;
pub const HEADER_SIZE: usize = 64;
pub const MAX_FRAME_SIZE: usize = 111424;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}
impl Image {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.width > 0 && self.height > 0,
            "image dimensions must be nonzero"
        );
        anyhow::ensure!(
            self.width.checked_mul(self.height) == Some(self.pixels.len()),
            "image pixel count does not match dimensions"
        );
        Ok(())
    }
    pub fn save_png(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        self.save_png_with_metadata(path.as_ref(), None)
    }
    pub(crate) fn save_png_with_metadata(
        &self,
        path: &Path,
        metadata: Option<&crate::metadata::CaptureMetadata>,
    ) -> anyhow::Result<()> {
        use std::io::Write;
        self.validate()?;
        let metadata_text = metadata.map(serde_json::to_string).transpose()?;
        anyhow::ensure!(
            metadata_text
                .as_ref()
                .is_none_or(|text| text.len() <= 16_384),
            "image metadata exceeds 16 KiB"
        );
        let width = u32::try_from(self.width)?;
        let height = u32::try_from(self.height)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        {
            let mut encoder = png::Encoder::new(&mut file, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            if let (Some(metadata), Some(text)) = (metadata, metadata_text) {
                encoder.set_pixel_dims(metadata.resolution.pixel_dimensions());
                encoder.add_itxt_chunk(crate::metadata::PNG_METADATA_KEY.into(), text)?;
            }
            let mut writer = encoder.write_header()?;
            writer.write_image_data(&self.pixels)?;
            writer.finish()?;
        }
        file.flush()?;
        Ok(())
    }
}
#[derive(Debug)]
pub struct InvalidFrame(pub &'static str);
impl fmt::Display for InvalidFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for InvalidFrame {}
struct Span {
    flags: u8,
    source: usize,
    samples: usize,
}

fn layout(bytes: &[u8]) -> Result<Vec<Span>, InvalidFrame> {
    if bytes.len() < HEADER_SIZE || bytes.len() > MAX_FRAME_SIZE {
        return Err(InvalidFrame("frame envelope length is invalid"));
    }
    let rows = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    if rows == 0 || rows > HEIGHT || bytes.len() < HEADER_SIZE + rows * WIDTH {
        return Err(InvalidFrame("source rows exceed available sensor data"));
    }
    let mut spans = Vec::with_capacity(15);
    let (mut source_rows, mut display_rows) = (0, 0);
    for descriptor in bytes[16..46].chunks_exact(2) {
        if source_rows == rows {
            break;
        }
        let count = usize::from(descriptor[1]);
        if count == 0 {
            break;
        }
        if source_rows + count > rows || display_rows + count > HEIGHT {
            return Err(InvalidFrame(
                "descriptor exceeds source or display capacity",
            ));
        }
        spans.push(Span {
            flags: descriptor[0],
            source: source_rows * WIDTH,
            samples: count * WIDTH,
        });
        display_rows += count;
        if descriptor[0] & 1 == 0 {
            source_rows += count;
        }
    }
    if source_rows != rows {
        return Err(InvalidFrame("descriptors do not account for source rows"));
    }
    Ok(spans)
}

fn appears_scrambled(source: &[u8]) -> bool {
    if source.len() < WIDTH * 2 {
        return false;
    }
    let sums: Vec<i64> = source[..WIDTH]
        .iter()
        .zip(&source[WIDTH..WIDTH * 2])
        .map(|(&a, &b)| i64::from(a) + i64::from(b))
        .collect();
    let mean = sums.iter().sum::<i64>() / WIDTH as i64;
    sums.iter().map(|s| (s - mean) * (s - mean)).sum::<i64>() / WIDTH as i64 >= 5000
}

fn step(state: u32) -> u32 {
    (state >> 1) | (((state & 0x9248_144d).count_ones() & 1) << 31)
}
fn mask(state: u32) -> u8 {
    [4, 8, 11, 14, 18, 21, 24, 29]
        .iter()
        .enumerate()
        .fold(0u8, |byte, (out, input)| {
            byte | (((state >> input) & 1) as u8) << out
        })
}

pub fn decode_frame(
    bytes: &[u8],
    rotate180: bool,
    key: impl FnMut(u8) -> anyhow::Result<u32>,
) -> anyhow::Result<Image> {
    Ok(decode_frame_with_metadata(bytes, rotate180, key)?.image)
}

/// Decode with padding/processing provenance. USB identity does not prove PPI.
pub fn decode_frame_with_metadata(
    bytes: &[u8],
    rotate180: bool,
    mut key: impl FnMut(u8) -> anyhow::Result<u32>,
) -> anyhow::Result<crate::metadata::CapturedImage> {
    use crate::metadata::{CaptureMetadata, CapturedImage, ProcessingStep, Region, Resolution};
    // Complete structural validation precedes all external key operations.
    let spans = layout(bytes)?;
    let rows = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    let mut source = bytes[HEADER_SIZE..HEADER_SIZE + rows * WIDTH].to_vec();
    let scrambled = appears_scrambled(&source);
    let mut index = bytes[6];
    let mut state = if scrambled { key(index)? } else { 0 };
    for span in &spans {
        if scrambled {
            if span.flags & 0x80 != 0 {
                index = index.wrapping_add(1);
                state = key(index)?;
            }
            match span.flags & 6 {
                0 => {
                    for _ in 0..span.samples {
                        state = step(state);
                    }
                }
                2 => {
                    let data = &mut source[span.source..span.source + span.samples];
                    for i in 0..data.len() {
                        data[i] = if i + 1 == data.len() {
                            0
                        } else {
                            data[i + 1] ^ mask(state)
                        };
                        state = step(state);
                    }
                }
                _ => {}
            }
        }
    }
    // Overlapping spans share the final sensor values, so expand only after
    // every descriptor has finished transforming that sensor buffer.
    let mut pixels = vec![255; WIDTH * HEIGHT];
    let mut destination = 0;
    for span in spans {
        for (&sample, output) in source[span.source..span.source + span.samples]
            .iter()
            .zip(&mut pixels[destination..destination + span.samples])
        {
            *output = !sample;
        }
        destination += span.samples;
    }
    if rotate180 {
        pixels.reverse();
    }
    let populated_rows = destination / WIDTH;
    let mut processing = vec![
        ProcessingStep::DescriptorExpansion,
        ProcessingStep::IntensityInversion,
    ];
    if rotate180 {
        processing.push(ProcessingStep::Rotate180);
    }
    Ok(CapturedImage {
        image: Image {
            width: WIDTH,
            height: HEIGHT,
            pixels,
        },
        metadata: CaptureMetadata {
            resolution: Resolution::Unknown,
            valid_region: Some(Region {
                x: 0,
                y: if rotate180 {
                    HEIGHT - populated_rows
                } else {
                    0
                },
                width: WIDTH,
                height: populated_rows,
            }),
            processing,
        },
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(rows: usize, descriptors: &[(u8, u8)]) -> Vec<u8> {
        let mut b = vec![0; HEADER_SIZE + rows * WIDTH];
        b[4..6].copy_from_slice(&(rows as u16).to_le_bytes());
        for (i, &(f, n)) in descriptors.iter().enumerate() {
            b[16 + 2 * i] = f;
            b[17 + 2 * i] = n;
        }
        b
    }
    #[test]
    fn full_capacity_and_normalization() {
        let b = frame(290, &[(0, 255), (0, 35)]);
        let im = decode_frame(&b, false, |_| panic!("plain data needs no key")).unwrap();
        assert_eq!(im.pixels, vec![255; WIDTH * HEIGHT]);
    }
    #[test]
    fn repeated_rows_and_rotation() {
        let mut b = frame(2, &[(1, 1), (0, 2)]);
        b[HEADER_SIZE] = 7;
        b[HEADER_SIZE + WIDTH] = 19;
        let im = decode_frame(&b, false, |_| unreachable!()).unwrap();
        assert_eq!(im.pixels[0], 248);
        assert_eq!(im.pixels[WIDTH], 248);
        assert_eq!(im.pixels[WIDTH * 2], 236);
        let rotated = decode_frame(&b, true, |_| unreachable!()).unwrap();
        assert!(rotated.pixels.iter().eq(im.pixels.iter().rev()));
    }
    #[test]
    fn validate_before_keys() {
        let mut b = frame(2, &[(2, 3)]);
        for (i, v) in b[HEADER_SIZE..].iter_mut().enumerate() {
            *v = if i % 2 == 0 { 0 } else { 255 };
        }
        assert!(
            decode_frame(&b, false, |_| panic!("invalid layout queried a key"))
                .unwrap_err()
                .is::<InvalidFrame>()
        );
    }
    #[test]
    fn callback_error_survives() {
        let mut b = frame(2, &[(2, 2)]);
        for (i, v) in b[HEADER_SIZE..].iter_mut().enumerate() {
            *v = if i % 2 == 0 { 0 } else { 255 };
        }
        let e = decode_frame(&b, false, |_| {
            Err(std::io::Error::other("key failed").into())
        })
        .unwrap_err();
        assert!(e.is::<std::io::Error>());
    }
    #[test]
    fn stream_known_vectors_and_descriptor_actions() {
        assert_eq!(step(1), 0x8000_0000);
        assert_eq!(step(0x8000_0000), 0xc000_0000);
        assert_eq!(mask(0xc179_a1bc), 0x63);
        let mut b = frame(2, &[(0, 1), (2, 1)]);
        for (i, v) in b[HEADER_SIZE..].iter_mut().enumerate() {
            *v = if i % 2 == 0 { 0 } else { 255 };
        }
        let im = decode_frame(&b, false, |_| Ok(1)).unwrap();
        assert_eq!(im.pixels[0], 255);
        assert_eq!(im.pixels[WIDTH], !(255 ^ 0x63));
        assert_eq!(im.pixels[WIDTH + 1], !0xb9);
        assert_eq!(im.pixels[WIDTH * 2 - 1], 255);
        b[16] = 0x86;
        b[6] = 255;
        let mut queried = Vec::new();
        let im = decode_frame(&b, false, |i| {
            queried.push(i);
            Ok(0)
        })
        .unwrap();
        assert_eq!(queried, vec![255, 0]);
        assert_eq!(im.pixels[WIDTH], 0);
        assert_eq!(im.pixels[WIDTH + 1], 255);
    }
    #[test]
    fn display_overflow_is_rejected_before_callback() {
        let b = frame(255, &[(1, 100), (0, 255)]);
        assert!(
            decode_frame(&b, false, |_| panic!("bad expansion queried key"))
                .unwrap_err()
                .is::<InvalidFrame>()
        );
    }
    #[test]
    fn reject_malformed() {
        for b in [
            vec![],
            frame(0, &[]),
            frame(291, &[(0, 255), (0, 36)]),
            frame(2, &[(0, 1)]),
        ] {
            assert!(
                decode_frame(&b, false, |_| unreachable!())
                    .unwrap_err()
                    .is::<InvalidFrame>()
            );
        }
    }
    #[test]
    fn png_refuses_bad_pixels_and_overwrite() {
        let bad = Image {
            width: 1,
            height: 2,
            pixels: vec![0],
        };
        let p = std::env::temp_dir().join(format!("uareu-fresh-{}.png", std::process::id()));
        assert!(bad.save_png(&p).is_err());
        let good = Image {
            width: 1,
            height: 1,
            pixels: vec![42],
        };
        good.save_png(&p).unwrap();
        assert!(good.save_png(&p).is_err());
        std::fs::remove_file(p).unwrap();
    }
}
