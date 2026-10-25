// SPDX-License-Identifier: GPL-3.0-or-later
//! ISO 19794-4:2005 FIR/010 single-view, raw 8-bit subset. Layout reference:
//! JMRTD FingerInfo/FingerImageInfo (see examples/engine/README.md). No PIV claim.
use crate::{
    frame::Image,
    metadata::{CapturedImage, ProcessingStep, Resolution, ResolutionProvenance},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRecordOptions {
    pub capture_device_id: u16,
    pub acquisition_level: u16,
    pub scale_units: u8,
    pub scan_resolution_x: u16,
    pub scan_resolution_y: u16,
    pub finger_position: u8,
    pub quality: u8,
    pub impression: u8,
}
pub struct DecodedImageRecord {
    pub image: CapturedImage,
    pub options: ImageRecordOptions,
}
fn validate(options: &ImageRecordOptions) -> Result<()> {
    ensure!(
        matches!(options.scale_units, 1 | 2)
            && options.scan_resolution_x > 0
            && options.scan_resolution_y > 0,
        "explicit scanning scale required"
    );
    ensure!(
        options.capture_device_id < 4096
            && options.finger_position <= 10
            && options.quality <= 100
            && matches!(options.impression, 0..=3 | 8),
        "unsupported acquisition declarations"
    );
    Ok(())
}
pub fn encode(image: &CapturedImage, options: &ImageRecordOptions) -> Result<Vec<u8>> {
    image.metadata.validate(&image.image)?;
    validate(options)?;
    let Resolution::Known { x_ppi, y_ppi, .. } = image.metadata.resolution else {
        anyhow::bail!("image record requires explicit image resolution");
    };
    let divisor = if options.scale_units == 1 { 1.0 } else { 2.54 };
    let density = |ppi: f64| -> Result<u16> {
        let n = ppi / divisor;
        ensure!(
            (1.0..=65535.0).contains(&n) && (n - n.round()).abs() < 0.0128,
            "resolution is not representable in selected scale units"
        );
        Ok(n.round() as u16)
    };
    let (rx, ry) = (density(x_ppi)?, density(y_ppi)?);
    ensure!(
        rx <= options.scan_resolution_x && ry <= options.scan_resolution_y,
        "image resolution exceeds caller-declared scanning resolution"
    );
    ensure!(
        image.image.width <= 65535
            && image.image.height <= 65535
            && image.image.pixels.len() <= 1024 * 1024,
        "image record geometry too large"
    );
    let length = 46 + image.image.pixels.len();
    let mut output = b"FIR\0".to_vec();
    output.extend_from_slice(b"010\0");
    output.extend_from_slice(&(length as u64).to_be_bytes()[2..]);
    output.extend_from_slice(&options.capture_device_id.to_be_bytes());
    output.extend_from_slice(&options.acquisition_level.to_be_bytes());
    output.extend_from_slice(&[1, options.scale_units]);
    for value in [options.scan_resolution_x, options.scan_resolution_y, rx, ry] {
        output.extend_from_slice(&value.to_be_bytes());
    }
    output.extend_from_slice(&[8, 0, 0, 0]);
    output.extend_from_slice(&((14 + image.image.pixels.len()) as u32).to_be_bytes());
    output.extend_from_slice(&[
        options.finger_position,
        1,
        1,
        options.quality,
        options.impression,
    ]);
    output.extend_from_slice(&(image.image.width as u16).to_be_bytes());
    output.extend_from_slice(&(image.image.height as u16).to_be_bytes());
    output.push(0);
    output.extend_from_slice(&image.image.pixels);
    ensure!(output.len() == length, "record length mismatch");
    Ok(output)
}
pub fn decode(bytes: &[u8]) -> Result<DecodedImageRecord> {
    ensure!(
        bytes.len() >= 47
            && bytes.len() <= 1024 * 1024 + 46
            && bytes.starts_with(b"FIR\0")
            && &bytes[4..8] == b"010\0",
        "unsupported image record envelope"
    );
    let mut size = [0u8; 8];
    size[2..].copy_from_slice(&bytes[8..14]);
    ensure!(
        u64::from_be_bytes(size) == bytes.len() as u64,
        "record length mismatch"
    );
    let word = |offset: usize| u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
    ensure!(
        bytes[18] == 1 && bytes[28] == 8 && bytes[29] == 0 && word(30) == 0,
        "only one raw 8-bit image is supported"
    );
    ensure!(
        u32::from_be_bytes(bytes[32..36].try_into()?) as usize == bytes.len() - 32
            && bytes[37] == 1
            && bytes[38] == 1
            && bytes[45] == 0,
        "unsupported fingerprint image record"
    );
    let options = ImageRecordOptions {
        capture_device_id: word(14),
        acquisition_level: word(16),
        scale_units: bytes[19],
        scan_resolution_x: word(20),
        scan_resolution_y: word(22),
        finger_position: bytes[36],
        quality: bytes[39],
        impression: bytes[40],
    };
    validate(&options)?;
    let (rx, ry) = (word(24), word(26));
    ensure!(
        rx > 0 && ry > 0 && rx <= options.scan_resolution_x && ry <= options.scan_resolution_y,
        "invalid declared resolution"
    );
    let (width, height) = (word(41) as usize, word(43) as usize);
    ensure!(
        width > 0 && height > 0 && width * height == bytes.len() - 46,
        "raw image length mismatch"
    );
    let factor = if options.scale_units == 1 { 1.0 } else { 2.54 };
    let mut image = CapturedImage::new(Image {
        width,
        height,
        pixels: bytes[46..].to_vec(),
    });
    image.metadata.resolution = Resolution::Known {
        x_ppi: rx as f64 * factor,
        y_ppi: ry as f64 * factor,
        provenance: ResolutionProvenance::ImageRecordHeaderDeclaration,
    };
    image
        .metadata
        .processing
        .push(ProcessingStep::ImageRecordImport);
    image.metadata.validate(&image.image)?;
    Ok(DecodedImageRecord { image, options })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_record_preserves_pixels_and_rejects_unknown_scale_or_compression() {
        let mut image = CapturedImage::new(Image {
            width: 16,
            height: 16,
            pixels: (0..256).map(|i| i as u8).collect(),
        });
        let options = ImageRecordOptions {
            capture_device_id: 0,
            acquisition_level: 0,
            scale_units: 1,
            scan_resolution_x: 500,
            scan_resolution_y: 500,
            finger_position: 0,
            quality: 0,
            impression: 0,
        };
        assert!(encode(&image, &options).is_err());
        image.metadata.resolution = Resolution::declared(500.0, 500.0).unwrap();
        let bytes = encode(&image, &options).unwrap();
        assert_eq!(bytes.len(), 302);
        assert_eq!(
            &bytes[..8],
            b"FIR\0".iter().chain(b"010\0").copied().collect::<Vec<_>>()
        );
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.image.image, image.image);
        assert_eq!(decoded.options.scan_resolution_x, 500);
        let mut excessive_scale = bytes.clone();
        excessive_scale[19] = 2;
        excessive_scale[20..28].fill(255);
        assert!(decode(&excessive_scale).is_err());
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err());
        }
        let mut compressed = bytes;
        compressed[29] = 2;
        assert!(decode(&compressed).is_err());
    }
}
