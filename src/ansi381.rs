// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded ANSI INCITS 381:2004 raw8 FIR/010 transport, independently verified
//! against NIST BiomDI. CBEFF fields/compliance are caller declarations, not PIV claims.
use crate::{
    image_record::{ImageCompression, ImageView, MultiImageRecord},
    metadata::CapturedImage,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

const MAX_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ansi381Record {
    /// High16 bits: registered product owner; low16 bits: product type.
    pub cbeff_product_id: u32,
    /// Header declaration only; this API does not certify scanner compliance.
    pub appendix_f_compliant: bool,
    /// Common image declarations; ANSI permits quality254 (undefined) in addition
    /// to1..100. Calling ISO methods on this field does not convert standards.
    pub record: MultiImageRecord,
}
impl Ansi381Record {
    fn normalized_common(&self) -> Result<MultiImageRecord> {
        ensure!(
            self.cbeff_product_id >> 16 != 0,
            "ANSI product owner must be nonzero"
        );
        ensure!(
            self.record.compression == ImageCompression::Raw8Bit,
            "ANSI adapter supports raw8 only"
        );
        ensure!(
            matches!(self.record.acquisition_level, 10 | 20 | 30 | 31 | 40 | 41),
            "unsupported ANSI acquisition level"
        );
        ensure!(
            self.record.scan_resolution_x <= 1000 && self.record.scan_resolution_y <= 1000,
            "ANSI adapter scanning resolution exceeds1000 units limit"
        );
        ensure!(
            (1..=255).contains(&self.record.images.len()),
            "ANSI requires1..255 images"
        );
        for image in &self.record.images {
            ensure!(image.data.len() <= 1024 * 1024, "ANSI raw view exceeds1MiB");
        }
        let mut common = self.record.clone();
        for image in &mut common.images {
            ensure!(
                (1..=100).contains(&image.quality) || image.quality == 254,
                "invalid ANSI image quality"
            );
            ensure!(
                matches!(image.impression, 0..=3 | 7 | 8),
                "unsupported ANSI impression"
            );
            // Reuse common geometry/view validation without promoting undefined
            // ANSI quality254 into a valid ISO quality declaration.
            if image.quality == 254 {
                image.quality = 0;
            }
        }
        common.validate()?;
        Ok(common)
    }
    pub fn validate(&self) -> Result<()> {
        self.normalized_common()?;
        let length = 36
            + self
                .record
                .images
                .iter()
                .map(|v| 14 + v.data.len())
                .sum::<usize>();
        ensure!(length <= MAX_BYTES, "ANSI record exceeds32MiB limit");
        Ok(())
    }
    /// Extract one raw view while retaining declared record scale/provenance.
    pub fn raw_image(&self, view: usize) -> Result<CapturedImage> {
        self.validate()?;
        self.normalized_common()?.raw_image(view)
    }
    pub fn edit(&mut self, edit: impl FnOnce(&mut Self)) -> Result<()> {
        let mut candidate = self.clone();
        edit(&mut candidate);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
}
pub fn encode(value: &Ansi381Record) -> Result<Vec<u8>> {
    value.validate()?;
    let r = &value.record;
    let length = 36 + r.images.iter().map(|v| 14 + v.data.len()).sum::<usize>();
    let mut out = Vec::with_capacity(length);
    out.extend_from_slice(b"FIR\0");
    out.extend_from_slice(b"010\0");
    out.extend_from_slice(&(length as u64).to_be_bytes()[2..]);
    out.extend_from_slice(&value.cbeff_product_id.to_be_bytes());
    let device = r.capture_device_id
        | if value.appendix_f_compliant {
            0x8000
        } else {
            0
        };
    out.extend_from_slice(&device.to_be_bytes());
    out.extend_from_slice(&r.acquisition_level.to_be_bytes());
    out.extend_from_slice(&[r.images.len() as u8, r.scale_units]);
    for word in [
        r.scan_resolution_x,
        r.scan_resolution_y,
        r.image_resolution_x,
        r.image_resolution_y,
    ] {
        out.extend_from_slice(&word.to_be_bytes());
    }
    out.extend_from_slice(&[8, 0, 0, 0]);
    for v in &r.images {
        out.extend_from_slice(&((14 + v.data.len()) as u32).to_be_bytes());
        out.extend_from_slice(&[
            v.finger_position,
            v.view_count,
            v.view_number,
            v.quality,
            v.impression,
        ]);
        out.extend_from_slice(&v.width.to_be_bytes());
        out.extend_from_slice(&v.height.to_be_bytes());
        out.push(0);
        out.extend_from_slice(&v.data);
    }
    Ok(out)
}
pub fn decode(bytes: &[u8]) -> Result<Ansi381Record> {
    ensure!(
        (51..=MAX_BYTES).contains(&bytes.len())
            && bytes.starts_with(b"FIR\0")
            && &bytes[4..8] == b"010\0",
        "unsupported ANSI FIR/010 envelope"
    );
    let mut length = [0; 8];
    length[2..].copy_from_slice(&bytes[8..14]);
    ensure!(
        u64::from_be_bytes(length) == bytes.len() as u64,
        "ANSI record length mismatch"
    );
    let word = |i| u16::from_be_bytes([bytes[i], bytes[i + 1]]);
    let device = word(18);
    ensure!(
        device & 0x7000 == 0 && bytes[32] == 8 && bytes[33] == 0 && word(34) == 0,
        "unsupported ANSI flags/depth/compression"
    );
    let mut record = MultiImageRecord {
        capture_device_id: device & 0x0fff,
        acquisition_level: word(20),
        scale_units: bytes[23],
        scan_resolution_x: word(24),
        scan_resolution_y: word(26),
        image_resolution_x: word(28),
        image_resolution_y: word(30),
        compression: ImageCompression::Raw8Bit,
        images: Vec::with_capacity(usize::from(bytes[22])),
    };
    let mut offset = 36usize;
    for _ in 0..bytes[22] {
        let header = bytes
            .get(offset..offset + 14)
            .ok_or_else(|| anyhow::anyhow!("truncated ANSI view header"))?;
        let length = u32::from_be_bytes(header[..4].try_into()?) as usize;
        ensure!(
            (15..=14 + 1024 * 1024).contains(&length),
            "ANSI raw view length outside limits"
        );
        let end = offset
            .checked_add(length)
            .ok_or_else(|| anyhow::anyhow!("ANSI view overflow"))?;
        let payload = bytes
            .get(offset + 14..end)
            .ok_or_else(|| anyhow::anyhow!("truncated ANSI image"))?;
        ensure!(header[13] == 0, "ANSI view reserved byte must be zero");
        record.images.push(ImageView {
            finger_position: header[4],
            view_count: header[5],
            view_number: header[6],
            quality: header[7],
            impression: header[8],
            width: u16::from_be_bytes(header[9..11].try_into()?),
            height: u16::from_be_bytes(header[11..13].try_into()?),
            data: payload.to_vec(),
        });
        offset = end;
    }
    ensure!(
        offset == bytes.len(),
        "ANSI trailing data/view count mismatch"
    );
    let value = Ansi381Record {
        cbeff_product_id: u32::from_be_bytes(bytes[14..18].try_into()?),
        appendix_f_compliant: device & 0x8000 != 0,
        record,
    };
    value.validate()?;
    Ok(value)
}
