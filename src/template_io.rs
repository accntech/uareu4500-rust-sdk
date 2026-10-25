// SPDX-License-Identifier: GPL-3.0-or-later
//! Strict minutiae records, with legacy no-extension APIs and additive
//! multi-view and opaque-extension transport APIs. Format references: Robert Važan's CC BY 4.0 summaries at
//! templates.machinezoo.com (see examples/engine/README.md).
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TemplateFormat {
    Iso19794_2_2005,
    Ansi378_2004,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Minutia {
    pub x: u16,
    pub y: u16,
    pub kind: u8,
    pub direction_clockwise: f64,
    pub quality: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MinutiaeRecord {
    pub width: u16,
    pub height: u16,
    pub pixels_per_cm_x: u16,
    pub pixels_per_cm_y: u16,
    pub finger_position: u8,
    pub impression: u8,
    pub quality: u8,
    pub vendor_id: u16,
    pub subformat: u16,
    pub device_info: u16,
    pub minutiae: Vec<Minutia>,
}
impl MinutiaeRecord {
    fn validate(&self, format: TemplateFormat) -> Result<()> {
        ensure!(
            self.width > 0
                && self.height > 0
                && self.pixels_per_cm_x > 0
                && self.pixels_per_cm_y > 0,
            "invalid record geometry/scale"
        );
        ensure!(
            self.minutiae.len() <= 255 && self.finger_position <= 10 && self.quality <= 100,
            "unsupported single-view record"
        );
        ensure!(
            matches!(self.impression, 0..=3 | 8),
            "unsupported impression type"
        );
        if format == TemplateFormat::Ansi378_2004 {
            ensure!(
                self.vendor_id != 0,
                "ANSI requires an explicit vendor identifier"
            );
        }
        for m in &self.minutiae {
            ensure!(
                m.x < self.width
                    && m.y < self.height
                    && m.x < 16384
                    && m.y < 16384
                    && m.kind <= 2
                    && m.quality <= 100
                    && m.direction_clockwise.is_finite()
                    && (0.0..TAU).contains(&m.direction_clockwise),
                "invalid minutia"
            );
        }
        Ok(())
    }
}
pub fn encode(record: &MinutiaeRecord, format: TemplateFormat) -> Result<Vec<u8>> {
    record.validate(format)?;
    let ansi = format == TemplateFormat::Ansi378_2004;
    let length = if ansi { 32 } else { 30 } + record.minutiae.len() * 6;
    let mut bytes = b"FMR\0 20\0".to_vec();
    if ansi {
        bytes.extend_from_slice(&(length as u16).to_be_bytes());
        bytes.extend_from_slice(&record.vendor_id.to_be_bytes());
        bytes.extend_from_slice(&record.subformat.to_be_bytes());
    } else {
        bytes.extend_from_slice(&(length as u32).to_be_bytes());
    }
    for value in [
        record.device_info,
        record.width,
        record.height,
        record.pixels_per_cm_x,
        record.pixels_per_cm_y,
    ] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(&[
        1,
        0,
        record.finger_position,
        record.impression,
        record.quality,
        record.minutiae.len() as u8,
    ]);
    for m in &record.minutiae {
        bytes.extend_from_slice(&(m.x | ((m.kind as u16) << 14)).to_be_bytes());
        bytes.extend_from_slice(&m.y.to_be_bytes());
        let angle = (-m.direction_clockwise).rem_euclid(TAU);
        let quantized = if ansi {
            (angle / TAU * 180.0).ceil() as u32 % 180
        } else {
            (angle / TAU * 256.0).round() as u32 & 255
        };
        bytes.extend_from_slice(&[quantized as u8, m.quality]);
    }
    bytes.extend_from_slice(&[0, 0]);
    ensure!(bytes.len() == length, "record length mismatch");
    Ok(bytes)
}
pub fn decode(bytes: &[u8], format: TemplateFormat) -> Result<MinutiaeRecord> {
    ensure!(
        bytes.len() >= 30 && bytes.len() <= 65535 && bytes.starts_with(b"FMR\0 20\0"),
        "unsupported template envelope/version"
    );
    let mut offset = 8;
    let word = |offset: &mut usize| -> Result<u16> {
        let slice = bytes
            .get(*offset..*offset + 2)
            .ok_or_else(|| anyhow::anyhow!("truncated template"))?;
        *offset += 2;
        Ok(u16::from_be_bytes([slice[0], slice[1]]))
    };
    let ansi = format == TemplateFormat::Ansi378_2004;
    let (length, vendor_id, subformat) = if ansi {
        let short = word(&mut offset)?;
        let length = if short == 0 {
            let hi = word(&mut offset)? as usize;
            let lo = word(&mut offset)? as usize;
            (hi << 16) | lo
        } else {
            short as usize
        };
        (length, word(&mut offset)?, word(&mut offset)?)
    } else {
        let hi = word(&mut offset)? as usize;
        let lo = word(&mut offset)? as usize;
        ((hi << 16) | lo, 0, 0)
    };
    ensure!(length == bytes.len(), "template length mismatch");
    let device_info = word(&mut offset)?;
    let width = word(&mut offset)?;
    let height = word(&mut offset)?;
    let pixels_per_cm_x = word(&mut offset)?;
    let pixels_per_cm_y = word(&mut offset)?;
    let view = bytes.get(offset..offset + 6).context("truncated view")?;
    offset += 6;
    ensure!(
        view[0] == 1 && view[1] == 0 && view[3] & 0xf0 == 0,
        "only one view with zero view index/reserved fields is supported"
    );
    let mut minutiae = Vec::new();
    for _ in 0..view[5] {
        let x = word(&mut offset)?;
        let y = word(&mut offset)?;
        ensure!(y & 0xc000 == 0, "reserved minutia bits set");
        let tail = bytes.get(offset..offset + 2).context("truncated minutia")?;
        offset += 2;
        ensure!(!ansi || tail[0] < 180, "invalid ANSI angle");
        let counterclockwise = if ansi {
            ((2 * tail[0] as i32 - 1 + 360) % 360) as f64 / 360.0 * TAU
        } else {
            tail[0] as f64 / 256.0 * TAU
        };
        minutiae.push(Minutia {
            x: x & 0x3fff,
            y,
            kind: (x >> 14) as u8,
            direction_clockwise: (-counterclockwise).rem_euclid(TAU),
            quality: tail[1],
        });
    }
    ensure!(
        word(&mut offset)? == 0 && offset == bytes.len(),
        "extensions/trailing data are unsupported"
    );
    let record = MinutiaeRecord {
        width,
        height,
        pixels_per_cm_x,
        pixels_per_cm_y,
        finger_position: view[2],
        impression: view[3],
        quality: view[4],
        vendor_id,
        subformat,
        device_info,
        minutiae,
    };
    record.validate(format)?;
    Ok(record)
}

/// One fingerprint view. View numbers start at zero for each finger position and
/// increase consecutively in record order, even when positions are interleaved.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FingerprintView {
    pub finger_position: u8,
    pub view_number: u8,
    pub impression: u8,
    pub quality: u8,
    pub minutiae: Vec<Minutia>,
}

/// Header metadata shared by every view in an ANSI 378:2004 or ISO 19794-2:2005
/// record. Extensions are deliberately unsupported and never silently discarded.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MultiViewRecord {
    pub width: u16,
    pub height: u16,
    pub pixels_per_cm_x: u16,
    pub pixels_per_cm_y: u16,
    pub vendor_id: u16,
    pub subformat: u16,
    pub device_info: u16,
    pub views: Vec<FingerprintView>,
}

impl From<MinutiaeRecord> for MultiViewRecord {
    fn from(record: MinutiaeRecord) -> Self {
        Self {
            width: record.width,
            height: record.height,
            pixels_per_cm_x: record.pixels_per_cm_x,
            pixels_per_cm_y: record.pixels_per_cm_y,
            vendor_id: record.vendor_id,
            subformat: record.subformat,
            device_info: record.device_info,
            views: vec![FingerprintView {
                finger_position: record.finger_position,
                view_number: 0,
                impression: record.impression,
                quality: record.quality,
                minutiae: record.minutiae,
            }],
        }
    }
}

impl MultiViewRecord {
    /// Validate the supported format subset before serializing or committing edits.
    pub fn validate(&self, format: TemplateFormat) -> Result<()> {
        ensure!(self.views.len() <= 176, "too many fingerprint views");
        ensure!(self.device_info & 0x7000 == 0, "reserved device bits set");
        if format == TemplateFormat::Iso19794_2_2005 {
            ensure!(
                self.pixels_per_cm_x >= 99 && self.pixels_per_cm_y >= 99,
                "ISO resolution must be at least 99 pixels/cm"
            );
        }
        // Reuse the legacy geometry/minutia checks, including an empty record
        // so zero-view envelopes still validate their header.
        let mut single = MinutiaeRecord {
            width: self.width,
            height: self.height,
            pixels_per_cm_x: self.pixels_per_cm_x,
            pixels_per_cm_y: self.pixels_per_cm_y,
            finger_position: 0,
            impression: 0,
            quality: 0,
            vendor_id: self.vendor_id,
            subformat: self.subformat,
            device_info: self.device_info,
            minutiae: Vec::new(),
        };
        single.validate(format)?;
        let mut next_number = [0u8; 11];
        for view in &self.views {
            ensure!(
                view.finger_position <= 10 && view.view_number <= 15,
                "invalid finger position/view number"
            );
            let next = &mut next_number[usize::from(view.finger_position)];
            ensure!(
                view.view_number == *next,
                "view numbers must start at zero and increase consecutively per finger"
            );
            *next += 1;
            let contactless = format == TemplateFormat::Ansi378_2004 && view.impression == 9;
            single.finger_position = view.finger_position;
            single.impression = if contactless { 0 } else { view.impression };
            single.quality = view.quality;
            single.minutiae.clone_from(&view.minutiae);
            single.validate(format)?;
        }
        Ok(())
    }

    /// Apply header, view-list, or minutia edits transactionally. Failed validation
    /// leaves the original record intact; callers control numbering when changing
    /// the view list. No matching/extraction or format conversion is implied.
    pub fn edit(&mut self, format: TemplateFormat, edit: impl FnOnce(&mut Self)) -> Result<()> {
        let mut candidate = self.clone();
        edit(&mut candidate);
        candidate.validate(format)?;
        *self = candidate;
        Ok(())
    }

    /// Edit one view and its minutiae transactionally, checking the whole record.
    pub fn edit_view(
        &mut self,
        index: usize,
        format: TemplateFormat,
        edit: impl FnOnce(&mut FingerprintView),
    ) -> Result<()> {
        ensure!(index < self.views.len(), "view index out of range");
        self.edit(format, |record| edit(&mut record.views[index]))
    }
}

/// Encode zero or more views. ANSI uses its extended length form above 65535
/// bytes. The legacy `encode` continues to emit exactly its original single view.
pub fn encode_views(record: &MultiViewRecord, format: TemplateFormat) -> Result<Vec<u8>> {
    record.validate(format)?;
    let ansi = format == TemplateFormat::Ansi378_2004;
    let short_length = if ansi { 26 } else { 24 }
        + record
            .views
            .iter()
            .map(|v| 6 + 6 * v.minutiae.len())
            .sum::<usize>();
    let extended = ansi && short_length > usize::from(u16::MAX);
    let length = short_length + if extended { 4 } else { 0 };
    let mut bytes = b"FMR\0 20\0".to_vec();
    if ansi {
        if extended {
            bytes.extend_from_slice(&[0, 0]);
            bytes.extend_from_slice(&u32::try_from(length)?.to_be_bytes());
        } else {
            bytes.extend_from_slice(&u16::try_from(length)?.to_be_bytes());
        }
        bytes.extend_from_slice(&record.vendor_id.to_be_bytes());
        bytes.extend_from_slice(&record.subformat.to_be_bytes());
    } else {
        bytes.extend_from_slice(&u32::try_from(length)?.to_be_bytes());
    }
    for value in [
        record.device_info,
        record.width,
        record.height,
        record.pixels_per_cm_x,
        record.pixels_per_cm_y,
    ] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(&[record.views.len() as u8, 0]);
    for view in &record.views {
        bytes.extend_from_slice(&[
            view.finger_position,
            view.view_number << 4 | view.impression,
            view.quality,
            view.minutiae.len() as u8,
        ]);
        for m in &view.minutiae {
            bytes.extend_from_slice(&(m.x | u16::from(m.kind) << 14).to_be_bytes());
            bytes.extend_from_slice(&m.y.to_be_bytes());
            let angle = (-m.direction_clockwise).rem_euclid(TAU);
            let quantized = if ansi {
                (angle / TAU * 180.0).ceil() as u32 % 180
            } else {
                (angle / TAU * 256.0).round() as u32 & 255
            };
            bytes.extend_from_slice(&[quantized as u8, m.quality]);
        }
        bytes.extend_from_slice(&[0, 0]);
    }
    ensure!(bytes.len() == length, "record length mismatch");
    Ok(bytes)
}

/// Decode the no-extension multi-view subset. Format selection is explicit;
/// unsupported versions, reserved bits, extension payloads and trailing bytes fail.
pub fn decode_views(bytes: &[u8], format: TemplateFormat) -> Result<MultiViewRecord> {
    // 176 valid views of at most 255 minutiae bounds this subset without allocating
    // according to an untrusted 32-bit total length.
    ensure!(
        bytes.len() <= 30 + 176 * (6 + 255 * 6) && bytes.starts_with(b"FMR\0 20\0"),
        "unsupported template envelope/version"
    );
    let mut offset = 8;
    let word = |offset: &mut usize| -> Result<u16> {
        let slice = bytes
            .get(*offset..*offset + 2)
            .context("truncated template")?;
        *offset += 2;
        Ok(u16::from_be_bytes([slice[0], slice[1]]))
    };
    let ansi = format == TemplateFormat::Ansi378_2004;
    let (length, vendor_id, subformat) = if ansi {
        let short = word(&mut offset)?;
        let length = if short == 0 {
            (usize::from(word(&mut offset)?) << 16) | usize::from(word(&mut offset)?)
        } else {
            usize::from(short)
        };
        (length, word(&mut offset)?, word(&mut offset)?)
    } else {
        (
            (usize::from(word(&mut offset)?) << 16) | usize::from(word(&mut offset)?),
            0,
            0,
        )
    };
    ensure!(length == bytes.len(), "template length mismatch");
    let mut record = MultiViewRecord {
        device_info: word(&mut offset)?,
        width: word(&mut offset)?,
        height: word(&mut offset)?,
        pixels_per_cm_x: word(&mut offset)?,
        pixels_per_cm_y: word(&mut offset)?,
        vendor_id,
        subformat,
        views: Vec::new(),
    };
    let count = bytes
        .get(offset..offset + 2)
        .context("truncated view count")?;
    offset += 2;
    ensure!(
        count[0] <= 176 && count[1] == 0,
        "invalid view count/reserved byte"
    );
    for _ in 0..count[0] {
        let view = bytes.get(offset..offset + 4).context("truncated view")?;
        offset += 4;
        let mut minutiae = Vec::new();
        for _ in 0..view[3] {
            let x = word(&mut offset)?;
            let y = word(&mut offset)?;
            ensure!(y & 0xc000 == 0, "reserved minutia bits set");
            let tail = bytes.get(offset..offset + 2).context("truncated minutia")?;
            offset += 2;
            ensure!(!ansi || tail[0] < 180, "invalid ANSI angle");
            let counterclockwise = if ansi {
                ((2 * i32::from(tail[0]) - 1 + 360) % 360) as f64 / 360.0 * TAU
            } else {
                f64::from(tail[0]) / 256.0 * TAU
            };
            minutiae.push(Minutia {
                x: x & 0x3fff,
                y,
                kind: (x >> 14) as u8,
                direction_clockwise: (-counterclockwise).rem_euclid(TAU),
                quality: tail[1],
            });
        }
        ensure!(word(&mut offset)? == 0, "extensions are unsupported");
        record.views.push(FingerprintView {
            finger_position: view[0],
            view_number: view[1] >> 4,
            impression: view[1] & 15,
            quality: view[2],
            minutiae,
        });
    }
    ensure!(offset == bytes.len(), "trailing data is unsupported");
    record.validate(format)?;
    Ok(record)
}

/// An extension envelope with uninterpreted payload. Type and length fields are
/// checked; predefined ridge/core/quality payload semantics are not interpreted.
/// No payload conversion occurs; callers must supply payloads appropriate to
/// the explicitly selected format.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplateExtension {
    pub type_code: u16,
    pub data: Vec<u8>,
}

/// Additive extension-preserving transport. `extensions[i]` belongs to
/// `record.views[i]`; both vectors must have identical lengths. The original
/// no-extension APIs continue rejecting these payloads instead of dropping them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtendedMultiViewRecord {
    pub record: MultiViewRecord,
    pub extensions: Vec<Vec<TemplateExtension>>,
}
impl ExtendedMultiViewRecord {
    pub fn validate(&self, format: TemplateFormat) -> Result<()> {
        self.record.validate(format)?;
        ensure!(
            self.extensions.len() == self.record.views.len(),
            "extension/view count mismatch"
        );
        for extensions in &self.extensions {
            let mut total = 0usize;
            for extension in extensions {
                ensure!(
                    extension.type_code & 255 != 0
                        && (extension.type_code > 255
                            || extension.type_code <= 2
                            || (format == TemplateFormat::Iso19794_2_2005
                                && extension.type_code == 3)),
                    "reserved extension type"
                );
                total = total
                    .checked_add(4)
                    .and_then(|v| v.checked_add(extension.data.len()))
                    .context("extension length overflow")?;
                ensure!(
                    total <= usize::from(u16::MAX),
                    "extension block exceeds 65535 bytes"
                );
            }
        }
        Ok(())
    }
    /// Validate a complete candidate before committing header, view or payload edits.
    pub fn edit(&mut self, format: TemplateFormat, edit: impl FnOnce(&mut Self)) -> Result<()> {
        let mut candidate = self.clone();
        edit(&mut candidate);
        candidate.validate(format)?;
        *self = candidate;
        Ok(())
    }
}

fn extended_header(bytes: &[u8], format: TemplateFormat) -> Result<usize> {
    let ansi = format == TemplateFormat::Ansi378_2004;
    let header = if ansi && bytes.get(8..10) == Some(&[0, 0]) {
        30
    } else if ansi {
        26
    } else {
        24
    };
    ensure!(
        bytes.len() >= header && bytes.starts_with(b"FMR\0 20\0"),
        "unsupported extension record envelope"
    );
    let declared = if !ansi {
        u32::from_be_bytes(bytes[8..12].try_into()?) as usize
    } else if header == 30 {
        u32::from_be_bytes(bytes[10..14].try_into()?) as usize
    } else {
        usize::from(u16::from_be_bytes(bytes[8..10].try_into()?))
    };
    ensure!(declared == bytes.len(), "template length mismatch");
    Ok(header)
}
fn rewrite_total_length(bytes: &mut Vec<u8>, format: TemplateFormat) -> Result<()> {
    if format == TemplateFormat::Ansi378_2004 {
        let extended = bytes[8..10] == [0, 0];
        if !extended && bytes.len() > usize::from(u16::MAX) {
            bytes.splice(10..10, [0; 4]);
            bytes[8..10].copy_from_slice(&[0, 0]);
        } else if extended && bytes.len() - 4 <= usize::from(u16::MAX) {
            bytes.drain(10..14);
        }
        let length = bytes.len();
        if length > usize::from(u16::MAX) {
            bytes[10..14].copy_from_slice(&u32::try_from(length)?.to_be_bytes());
        } else {
            bytes[8..10].copy_from_slice(&u16::try_from(length)?.to_be_bytes());
        }
    } else {
        let length = u32::try_from(bytes.len())?;
        bytes[8..12].copy_from_slice(&length.to_be_bytes());
    }
    Ok(())
}

/// Encode extension envelopes without interpreting payload contents. ANSI block
/// lengths include their four-byte header; ISO block lengths exclude it.
pub fn encode_extended(
    record: &ExtendedMultiViewRecord,
    format: TemplateFormat,
) -> Result<Vec<u8>> {
    record.validate(format)?;
    let base = encode_views(&record.record, format)?;
    let header = extended_header(&base, format)?;
    let mut bytes = base[..header].to_vec();
    let mut offset = header;
    for (view, extensions) in record.record.views.iter().zip(&record.extensions) {
        let end = offset + 4 + 6 * view.minutiae.len();
        bytes.extend_from_slice(&base[offset..end]);
        offset = end + 2;
        let length = extensions.iter().map(|v| 4 + v.data.len()).sum::<usize>();
        bytes.extend_from_slice(&u16::try_from(length)?.to_be_bytes());
        for extension in extensions {
            bytes.extend_from_slice(&extension.type_code.to_be_bytes());
            let length = extension.data.len()
                + if format == TemplateFormat::Ansi378_2004 {
                    4
                } else {
                    0
                };
            bytes.extend_from_slice(&u16::try_from(length)?.to_be_bytes());
            bytes.extend_from_slice(&extension.data);
        }
    }
    rewrite_total_length(&mut bytes, format)?;
    Ok(bytes)
}

/// Decode bounded, ordered extension envelopes and preserve every payload byte.
/// At most 176 views and 65535 extension bytes/view are accepted. This is
/// transport validation, not validation of ridge-count/core/quality semantics.
pub fn decode_extended(bytes: &[u8], format: TemplateFormat) -> Result<ExtendedMultiViewRecord> {
    ensure!(
        bytes.len() <= 30 + 176 * (6 + 255 * 6 + 65535),
        "extension record exceeds supported limit"
    );
    let header = extended_header(bytes, format)?;
    let count = usize::from(bytes[header - 2]);
    ensure!(count <= 176, "too many fingerprint views");
    let mut base = bytes[..header].to_vec();
    let mut offset = header;
    let mut extensions = Vec::with_capacity(count);
    for _ in 0..count {
        let view = bytes.get(offset..offset + 4).context("truncated view")?;
        let core_end = offset + 4 + 6 * usize::from(view[3]);
        base.extend_from_slice(bytes.get(offset..core_end).context("truncated minutiae")?);
        let total = usize::from(u16::from_be_bytes(
            bytes
                .get(core_end..core_end + 2)
                .context("truncated extension length")?
                .try_into()?,
        ));
        offset = core_end + 2;
        let end = offset + total;
        ensure!(end <= bytes.len(), "truncated extensions");
        let mut blocks = Vec::new();
        while offset < end {
            let block = bytes
                .get(offset..offset + 4)
                .filter(|_| offset + 4 <= end)
                .context("truncated extension header")?;
            let type_code = u16::from_be_bytes(block[..2].try_into()?);
            let declared = usize::from(u16::from_be_bytes(block[2..4].try_into()?));
            let payload_length = if format == TemplateFormat::Ansi378_2004 {
                declared
                    .checked_sub(4)
                    .context("ANSI extension length below four")?
            } else {
                declared
            };
            offset += 4;
            let payload_end = offset + payload_length;
            ensure!(payload_end <= end, "extension length exceeds view block");
            blocks.push(TemplateExtension {
                type_code,
                data: bytes[offset..payload_end].to_vec(),
            });
            offset = payload_end;
        }
        extensions.push(blocks);
        base.extend_from_slice(&[0, 0]);
    }
    ensure!(offset == bytes.len(), "trailing extension record data");
    rewrite_total_length(&mut base, format)?;
    let value = ExtendedMultiViewRecord {
        record: decode_views(&base, format)?,
        extensions,
    };
    value.validate(format)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_single_view_layouts_roundtrip_and_reject_corruption() {
        let record = MinutiaeRecord {
            width: 384,
            height: 290,
            pixels_per_cm_x: 197,
            pixels_per_cm_y: 197,
            finger_position: 0,
            impression: 0,
            quality: 0,
            vendor_id: 0x0103,
            subformat: 0,
            device_info: 0,
            minutiae: vec![Minutia {
                x: 10,
                y: 20,
                kind: 1,
                direction_clockwise: 0.0,
                quality: 0,
            }],
        };
        for format in [
            TemplateFormat::Iso19794_2_2005,
            TemplateFormat::Ansi378_2004,
        ] {
            let bytes = encode(&record, format).unwrap();
            let parsed = decode(&bytes, format).unwrap();
            assert_eq!(parsed.minutiae[0].x, 10);
            assert_eq!(parsed.minutiae[0].y, 20);
            assert_eq!(parsed.minutiae[0].kind, 1);
            assert_eq!(parsed.pixels_per_cm_x, 197);
            assert!(
                parsed.minutiae[0]
                    .direction_clockwise
                    .min(TAU - parsed.minutiae[0].direction_clockwise)
                    < 0.02
            );
            for len in 0..bytes.len() {
                assert!(decode(&bytes[..len], format).is_err());
            }
            let mut trailing = bytes.clone();
            trailing.push(0);
            assert!(decode(&trailing, format).is_err());
        }
    }
}
