// SPDX-License-Identifier: GPL-3.0-or-later
//! Strict single-view/no-extension subset. Format references: Robert Važan's
//! CC BY 4.0 summaries at templates.machinezoo.com (see examples/engine/README.md).
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
