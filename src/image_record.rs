// SPDX-License-Identifier: GPL-3.0-or-later
//! ISO 19794-4:2005 FIR/010 image records, including additive multi-image APIs.
//! Legacy single-view raw 8-bit functions retain their contract. Layout reference:
//! JMRTD FingerInfo/FingerImageInfo (see examples/engine/README.md). No PIV claim.
use crate::{
    frame::Image,
    jpeg2000::Jpeg2000Codec,
    metadata::{CapturedImage, ProcessingStep, Resolution, ResolutionProvenance},
    wsq::WsqCodec,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

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
/// Supported ISO FIR/010 payload encoding. Raw pixels are validated as 8-bit
/// grayscale; compressed payload variants are explicitly opaque transport.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageCompression {
    Raw8Bit,
    /// ISO compression code 2. Bytes are transported unchanged, without WSQ
    /// parsing, decompression, or verification of declared dimensions/depth.
    OpaqueWsq,
    /// ISO compression code 4. Bytes are transported unchanged, without JPEG2000
    /// parsing, decompression, or verification of declared dimensions/depth.
    OpaqueJpeg2000,
}
impl ImageCompression {
    fn code(self) -> u8 {
        match self {
            Self::Raw8Bit => 0,
            Self::OpaqueWsq => 2,
            Self::OpaqueJpeg2000 => 4,
        }
    }
    fn from_code(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::Raw8Bit),
            2 => Ok(Self::OpaqueWsq),
            4 => Ok(Self::OpaqueJpeg2000),
            _ => anyhow::bail!("unsupported image compression"),
        }
    }
}
/// One image block, with one-based view identity within its finger position.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageView {
    pub finger_position: u8,
    pub view_count: u8,
    pub view_number: u8,
    pub quality: u8,
    pub impression: u8,
    pub width: u16,
    pub height: u16,
    pub data: Vec<u8>,
}
/// ISO 19794-4:2005 FIR/010 record with shared scale/encoding and multiple images.
/// This API supports single-finger positions (0..=10), 8-bit depth, at most 255
/// images, 1 MiB per raw image, 4 MiB per opaque compressed image, and 32 MiB
/// per complete record. Compressed header declarations are not codec-verified.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MultiImageRecord {
    pub capture_device_id: u16,
    pub acquisition_level: u16,
    pub scale_units: u8,
    pub scan_resolution_x: u16,
    pub scan_resolution_y: u16,
    pub image_resolution_x: u16,
    pub image_resolution_y: u16,
    pub compression: ImageCompression,
    pub images: Vec<ImageView>,
}
const MAX_MULTI_IMAGE_BYTES: usize = 32 * 1024 * 1024;
impl MultiImageRecord {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.images.is_empty() && self.images.len() <= 255,
            "invalid image count"
        );
        ensure!(
            self.capture_device_id < 4096 && matches!(self.scale_units, 1 | 2),
            "unsupported device/scale declaration"
        );
        ensure!(
            self.image_resolution_x > 0
                && self.image_resolution_y > 0
                && self.image_resolution_x <= self.scan_resolution_x
                && self.image_resolution_y <= self.scan_resolution_y,
            "invalid declared resolution"
        );
        let mut length = 32usize;
        let mut counts = [0usize; 11];
        let mut seen = [[false; 256]; 11];
        for image in &self.images {
            ensure!(
                image.finger_position <= 10
                    && image.quality <= 100
                    && matches!(image.impression, 0..=3 | 7..=9),
                "unsupported image declarations"
            );
            ensure!(
                image.width > 0 && image.height > 0 && !image.data.is_empty(),
                "invalid image geometry/payload"
            );
            match self.compression {
                ImageCompression::Raw8Bit => ensure!(
                    image.data.len() <= 1024 * 1024
                        && image.data.len() == usize::from(image.width) * usize::from(image.height),
                    "raw image length mismatch/limit exceeded"
                ),
                ImageCompression::OpaqueWsq | ImageCompression::OpaqueJpeg2000 => ensure!(
                    image.data.len() <= 4 * 1024 * 1024,
                    "compressed image exceeds 4 MiB transport limit"
                ),
            }
            ensure!(
                image.view_count > 0
                    && image.view_number > 0
                    && image.view_number <= image.view_count,
                "invalid view count/number"
            );
            let finger = usize::from(image.finger_position);
            let number = usize::from(image.view_number);
            ensure!(!seen[finger][number], "duplicate image view identity");
            seen[finger][number] = true;
            counts[finger] += 1;
            length = length
                .checked_add(14 + image.data.len())
                .ok_or_else(|| anyhow::anyhow!("record length overflow"))?;
        }
        ensure!(
            length <= MAX_MULTI_IMAGE_BYTES,
            "image record exceeds 32 MiB limit"
        );
        for image in &self.images {
            ensure!(
                counts[usize::from(image.finger_position)] == usize::from(image.view_count),
                "view count does not match record contents"
            );
        }
        Ok(())
    }
    /// Compress every raw view using the existing NBIS adapter and one shared
    /// deadline. The original is unchanged on any failure. Only declarations
    /// that represent 500 PPI at the header's integer unit precision are accepted.
    pub fn compress_wsq(&self, codec: &WsqCodec, bitrate: f64, deadline: Duration) -> Result<Self> {
        let start = Instant::now();
        validate_wsq_deadlines(codec, deadline)?;
        self.validate()?;
        ensure!(
            self.compression == ImageCompression::Raw8Bit,
            "WSQ compression requires raw images"
        );
        ensure!(
            bitrate.is_finite() && (0.1..=2.25).contains(&bitrate),
            "invalid WSQ bitrate"
        );
        let factor = if self.scale_units == 1 { 1.0 } else { 2.54 };
        let expected = (500.0_f64 / factor).round() as u16;
        ensure!(
            self.image_resolution_x == expected && self.image_resolution_y == expected,
            "WSQ adapter requires a 500-PPI image declaration"
        );
        // Preflight every view before invoking any external tool.
        ensure!(
            self.images.iter().all(|image| image.width >= 256
                && image.height >= 256
                && image.width <= 2048
                && image.height <= 2048),
            "unsupported WSQ image size"
        );
        let mut output = self.clone();
        output.compression = ImageCompression::OpaqueWsq;
        let mut output_length = 32usize;
        for (index, view) in output.images.iter_mut().enumerate() {
            let mut image = self.raw_image(index)?;
            // 197 pixels/cm is the rounded header representation of 500 PPI.
            // This supplies a codec declaration, not a measured resolution.
            image.metadata.resolution = Resolution::declared(500.0, 500.0)?;
            let bounded = bounded_wsq_codec(codec, start, deadline)?;
            view.data = bounded.encode(&image, bitrate)?;
            output_length += 14 + view.data.len();
            ensure!(
                output_length <= MAX_MULTI_IMAGE_BYTES,
                "compressed record exceeds 32 MiB limit"
            );
        }
        output.validate()?;
        ensure!(
            start.elapsed() < deadline,
            "image-record WSQ deadline exceeded"
        );
        Ok(output)
    }

    /// Decode every WSQ view using one shared deadline. Decoded geometry and any
    /// codec PPI declaration must agree with the record at header unit precision.
    /// Unknown codec PPI retains the record's explicit declaration. JPEG2000
    /// remains opaque and is rejected by this WSQ-specific operation.
    pub fn decompress_wsq(&self, codec: &WsqCodec, deadline: Duration) -> Result<Self> {
        let start = Instant::now();
        validate_wsq_deadlines(codec, deadline)?;
        self.validate()?;
        ensure!(
            self.compression == ImageCompression::OpaqueWsq,
            "WSQ decompression requires WSQ images"
        );
        let mut output = self.clone();
        output.compression = ImageCompression::Raw8Bit;
        let factor = if self.scale_units == 1 { 1.0 } else { 2.54 };
        let mut output_length = 32usize;
        for view in &mut output.images {
            let bounded = bounded_wsq_codec(codec, start, deadline)?;
            let decoded = bounded.decode(&view.data)?;
            ensure!(
                decoded.image.width == usize::from(view.width)
                    && decoded.image.height == usize::from(view.height),
                "decoded WSQ geometry conflicts with record"
            );
            if let Resolution::Known { x_ppi, y_ppi, .. } = decoded.metadata.resolution {
                ensure!(
                    (x_ppi / factor).round() == f64::from(self.image_resolution_x)
                        && (y_ppi / factor).round() == f64::from(self.image_resolution_y),
                    "decoded WSQ PPI conflicts with record"
                );
            }
            view.data = decoded.image.pixels;
            output_length += 14 + view.data.len();
            ensure!(
                output_length <= MAX_MULTI_IMAGE_BYTES,
                "decompressed record exceeds 32 MiB limit"
            );
        }
        output.validate()?;
        ensure!(
            start.elapsed() < deadline,
            "image-record WSQ deadline exceeded"
        );
        Ok(output)
    }

    /// Losslessly encode each raw view as a grayscale raw JPEG2000 codestream.
    /// Retain the record's declared scale; one deadline bounds the entire operation.
    pub fn compress_jpeg2000(&self, codec: &Jpeg2000Codec, deadline: Duration) -> Result<Self> {
        self.transform_jpeg2000(codec, deadline, true)
    }
    /// Decode supported grayscale raw JPEG2000 codestream views and verify every
    /// decoded geometry against the record. Header scale remains a declaration.
    pub fn decompress_jpeg2000(&self, codec: &Jpeg2000Codec, deadline: Duration) -> Result<Self> {
        self.transform_jpeg2000(codec, deadline, false)
    }
    fn transform_jpeg2000(
        &self,
        codec: &Jpeg2000Codec,
        deadline: Duration,
        encode: bool,
    ) -> Result<Self> {
        let start = Instant::now();
        ensure!(
            deadline > Duration::ZERO
                && deadline <= Duration::from_secs(300)
                && codec.timeout > Duration::ZERO
                && codec.timeout <= Duration::from_secs(300),
            "JPEG2000 record deadline out of range"
        );
        self.validate()?;
        ensure!(
            self.compression
                == if encode {
                    ImageCompression::Raw8Bit
                } else {
                    ImageCompression::OpaqueJpeg2000
                },
            "JPEG2000 record operation requires matching payload encoding"
        );
        ensure!(
            self.images.iter().all(|v| v.width <= 4096
                && v.height <= 4096
                && usize::from(v.width) * usize::from(v.height) <= 1024 * 1024),
            "JPEG2000 record geometry exceeds codec limits"
        );
        let mut output = self.clone();
        output.compression = if encode {
            ImageCompression::OpaqueJpeg2000
        } else {
            ImageCompression::Raw8Bit
        };
        let mut length = 32usize;
        for (index, view) in output.images.iter_mut().enumerate() {
            let remaining = deadline.saturating_sub(start.elapsed());
            ensure!(
                !remaining.is_zero(),
                "image-record JPEG2000 deadline exceeded"
            );
            let bounded = Jpeg2000Codec {
                timeout: codec.timeout.min(remaining),
                ..codec.clone()
            };
            view.data = if encode {
                bounded.encode(&self.raw_image(index)?)?
            } else {
                let decoded = bounded.decode(&view.data)?;
                ensure!(
                    decoded.image.width == usize::from(view.width)
                        && decoded.image.height == usize::from(view.height),
                    "decoded JPEG2000 geometry conflicts with record"
                );
                decoded.image.pixels
            };
            length += 14 + view.data.len();
            ensure!(
                length <= MAX_MULTI_IMAGE_BYTES,
                "JPEG2000 record exceeds 32 MiB limit"
            );
        }
        output.validate()?;
        ensure!(
            start.elapsed() < deadline,
            "image-record JPEG2000 deadline exceeded"
        );
        Ok(output)
    }

    /// Transactional header, payload and image-list edits. View count/identity
    /// must remain consistent; validation failures leave the original unchanged.
    pub fn edit(&mut self, edit: impl FnOnce(&mut Self)) -> Result<()> {
        let mut candidate = self.clone();
        edit(&mut candidate);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
    pub fn edit_image(&mut self, index: usize, edit: impl FnOnce(&mut ImageView)) -> Result<()> {
        ensure!(index < self.images.len(), "image index out of range");
        self.edit(|record| edit(&mut record.images[index]))
    }
    /// Import a raw image into capture metadata. Header resolution is recorded
    /// as a declaration, never as independently measured sensor resolution.
    pub fn raw_image(&self, index: usize) -> Result<CapturedImage> {
        self.validate()?;
        ensure!(
            self.compression == ImageCompression::Raw8Bit,
            "compressed image requires a codec"
        );
        let view = self
            .images
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("image index out of range"))?;
        let mut image = CapturedImage::new(Image {
            width: usize::from(view.width),
            height: usize::from(view.height),
            pixels: view.data.clone(),
        });
        let factor = if self.scale_units == 1 { 1.0 } else { 2.54 };
        image.metadata.resolution = Resolution::Known {
            x_ppi: f64::from(self.image_resolution_x) * factor,
            y_ppi: f64::from(self.image_resolution_y) * factor,
            provenance: ResolutionProvenance::ImageRecordHeaderDeclaration,
        };
        image
            .metadata
            .processing
            .push(ProcessingStep::ImageRecordImport);
        image.metadata.validate(&image.image)?;
        Ok(image)
    }
}
fn validate_wsq_deadlines(codec: &WsqCodec, deadline: Duration) -> Result<()> {
    ensure!(
        deadline > Duration::ZERO && deadline <= Duration::from_secs(300),
        "image-record WSQ deadline must be positive and at most 300 seconds"
    );
    ensure!(
        codec.timeout > Duration::ZERO && codec.timeout <= Duration::from_secs(300),
        "WSQ codec timeout must be positive and at most 300 seconds"
    );
    Ok(())
}
fn bounded_wsq_codec(codec: &WsqCodec, start: Instant, deadline: Duration) -> Result<WsqCodec> {
    let remaining = deadline.saturating_sub(start.elapsed());
    ensure!(!remaining.is_zero(), "image-record WSQ deadline exceeded");
    let mut bounded = codec.clone();
    bounded.timeout = codec.timeout.min(remaining);
    Ok(bounded)
}

/// Encode the supported multi-image ISO record subset. Legacy `encode` retains
/// its original single-image contract.
pub fn encode_images(record: &MultiImageRecord) -> Result<Vec<u8>> {
    record.validate()?;
    let length = 32
        + record
            .images
            .iter()
            .map(|image| 14 + image.data.len())
            .sum::<usize>();
    let mut output = b"FIR\0".to_vec();
    output.extend_from_slice(b"010\0");
    output.extend_from_slice(&(length as u64).to_be_bytes()[2..]);
    output.extend_from_slice(&record.capture_device_id.to_be_bytes());
    output.extend_from_slice(&record.acquisition_level.to_be_bytes());
    output.extend_from_slice(&[record.images.len() as u8, record.scale_units]);
    for value in [
        record.scan_resolution_x,
        record.scan_resolution_y,
        record.image_resolution_x,
        record.image_resolution_y,
    ] {
        output.extend_from_slice(&value.to_be_bytes());
    }
    output.extend_from_slice(&[8, record.compression.code(), 0, 0]);
    for image in &record.images {
        output.extend_from_slice(&u32::try_from(14 + image.data.len())?.to_be_bytes());
        output.extend_from_slice(&[
            image.finger_position,
            image.view_count,
            image.view_number,
            image.quality,
            image.impression,
        ]);
        output.extend_from_slice(&image.width.to_be_bytes());
        output.extend_from_slice(&image.height.to_be_bytes());
        output.push(0);
        output.extend_from_slice(&image.data);
    }
    Ok(output)
}
/// Decode length-bounded multi-image ISO records without invoking any codec.
pub fn decode_images(bytes: &[u8]) -> Result<MultiImageRecord> {
    ensure!(
        bytes.len() >= 47
            && bytes.len() <= MAX_MULTI_IMAGE_BYTES
            && bytes.starts_with(b"FIR\x00010\0"),
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
        bytes[28] == 8 && word(30) == 0,
        "unsupported image depth/reserved fields"
    );
    let mut record = MultiImageRecord {
        capture_device_id: word(14),
        acquisition_level: word(16),
        scale_units: bytes[19],
        scan_resolution_x: word(20),
        scan_resolution_y: word(22),
        image_resolution_x: word(24),
        image_resolution_y: word(26),
        compression: ImageCompression::from_code(bytes[29])?,
        images: Vec::new(),
    };
    let mut offset = 32usize;
    for _ in 0..bytes[18] {
        let header = bytes
            .get(offset..offset + 14)
            .ok_or_else(|| anyhow::anyhow!("truncated image header"))?;
        let block_length = u32::from_be_bytes(header[..4].try_into()?) as usize;
        ensure!(
            block_length >= 15 && header[13] == 0,
            "invalid image block/reserved byte"
        );
        let end = offset
            .checked_add(block_length)
            .ok_or_else(|| anyhow::anyhow!("image length overflow"))?;
        let data = bytes
            .get(offset + 14..end)
            .ok_or_else(|| anyhow::anyhow!("truncated image payload"))?;
        let payload_limit = if record.compression == ImageCompression::Raw8Bit {
            1024 * 1024
        } else {
            4 * 1024 * 1024
        };
        ensure!(data.len() <= payload_limit, "image payload limit exceeded");
        record.images.push(ImageView {
            finger_position: header[4],
            view_count: header[5],
            view_number: header[6],
            quality: header[7],
            impression: header[8],
            width: u16::from_be_bytes([header[9], header[10]]),
            height: u16::from_be_bytes([header[11], header[12]]),
            data: data.to_vec(),
        });
        offset = end;
    }
    ensure!(offset == bytes.len(), "trailing image record data");
    record.validate()?;
    Ok(record)
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
