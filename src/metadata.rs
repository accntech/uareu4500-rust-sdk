// SPDX-License-Identifier: GPL-3.0-or-later
//! Image metadata records declarations and processing, never biometric identity.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::BufReader, path::Path};

use crate::frame::Image;

pub(crate) const PNG_METADATA_KEY: &str = "uareu.metadata.v1";
// Read existing captures without dropping their resolution/processing provenance.
const LEGACY_PNG_METADATA_KEY: &str = "uareu4500.metadata.v1";
const MAX_PIXELS: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Resolution {
    Unknown,
    Known {
        x_ppi: f64,
        y_ppi: f64,
        provenance: ResolutionProvenance,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ResolutionProvenance {
    CallerDeclared,
    PngPhysicalDimensions,
    WsqCommentDeclaration,
    TemplateHeaderDeclaration { format: String },
    ImageRecordHeaderDeclaration,
    DeviceReported { evidence: String },
    DocumentedProfile { evidence: String },
}

impl Resolution {
    /// A caller declaration is not a hardware measurement.
    pub fn declared(x_ppi: f64, y_ppi: f64) -> Result<Self> {
        let resolution = Self::Known {
            x_ppi,
            y_ppi,
            provenance: ResolutionProvenance::CallerDeclared,
        };
        resolution.validate()?;
        Ok(resolution)
    }

    pub fn validate(&self) -> Result<()> {
        if let Self::Known {
            x_ppi,
            y_ppi,
            provenance,
        } = self
        {
            ensure!(
                [*x_ppi, *y_ppi]
                    .iter()
                    .all(|n| n.is_finite() && *n >= 1.0 && *n <= 100_000.0),
                "resolution must be finite and between 1 and 100000 PPI"
            );
            if let ResolutionProvenance::DeviceReported { evidence }
            | ResolutionProvenance::DocumentedProfile { evidence } = provenance
            {
                ensure!(
                    !evidence.trim().is_empty(),
                    "resolution provenance requires evidence"
                );
            }
        }
        Ok(())
    }

    pub(crate) fn pixel_dimensions(&self) -> Option<png::PixelDimensions> {
        match self {
            Self::Unknown => None,
            Self::Known { x_ppi, y_ppi, .. } => Some(png::PixelDimensions {
                xppu: (x_ppi / 0.0254).round() as u32,
                yppu: (y_ppi / 0.0254).round() as u32,
                unit: png::Unit::Meter,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingStep {
    DescriptorExpansion,
    IntensityInversion,
    Rotate180,
    ContrastStretch { lower: u8, upper: u8 },
    FlatFieldCorrection { profile_sha256: [u8; 32] },
    WsqDecompression,
    Jpeg2000Decompression,
    ImageRecordImport,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureMetadata {
    pub resolution: Resolution,
    /// Descriptor-populated area, not a segmented finger/foreground region.
    /// None means the padding/valid region is unknown, not that it is empty.
    pub valid_region: Option<Region>,
    pub processing: Vec<ProcessingStep>,
}

impl Default for CaptureMetadata {
    fn default() -> Self {
        Self {
            resolution: Resolution::Unknown,
            valid_region: None,
            processing: Vec::new(),
        }
    }
}

impl CaptureMetadata {
    pub fn validate(&self, image: &Image) -> Result<()> {
        image.validate()?;
        self.resolution.validate()?;
        if let Some(region) = self.valid_region {
            ensure!(
                region.width > 0
                    && region.height > 0
                    && region
                        .x
                        .checked_add(region.width)
                        .is_some_and(|n| n <= image.width)
                    && region
                        .y
                        .checked_add(region.height)
                        .is_some_and(|n| n <= image.height),
                "valid region exceeds image bounds or is empty"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CapturedImage {
    pub image: Image,
    pub metadata: CaptureMetadata,
}

impl CapturedImage {
    /// Create a metadata-bearing image without guessing resolution or padding.
    pub fn new(image: Image) -> Self {
        Self {
            image,
            metadata: CaptureMetadata::default(),
        }
    }

    /// Save without overwriting; records resolution and metadata declarations.
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<()> {
        self.metadata.validate(&self.image)?;
        self.image
            .save_png_with_metadata(path.as_ref(), Some(&self.metadata))
    }

    /// Read 8-bit grayscale PNG. Embedded metadata is a declaration, not proof.
    pub fn load_png(path: impl AsRef<Path>) -> Result<Self> {
        let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        decoder.set_limits(png::Limits { bytes: MAX_PIXELS });
        let mut reader = decoder.read_info().context("cannot read PNG")?;
        let info = reader.info();
        ensure!(
            info.color_type == png::ColorType::Grayscale && info.bit_depth == png::BitDepth::Eight,
            "quality input must be an 8-bit grayscale PNG"
        );
        ensure!(
            info.animation_control.is_none(),
            "animated PNG is not supported"
        );
        let width = info.width as usize;
        let height = info.height as usize;
        let length = width
            .checked_mul(height)
            .filter(|n| *n > 0 && *n <= MAX_PIXELS)
            .context("PNG exceeds the 16 MiB pixel limit")?;
        let physical = info.pixel_dims;
        let mut pixels = vec![0; length];
        reader
            .next_frame(&mut pixels)
            .context("cannot decode PNG pixels")?;
        reader.finish().context("cannot finish PNG")?;
        let mut embedded: Vec<_> = reader
            .info()
            .utf8_text
            .iter()
            .filter(|entry| {
                entry.keyword == PNG_METADATA_KEY || entry.keyword == LEGACY_PNG_METADATA_KEY
            })
            .cloned()
            .collect();
        ensure!(embedded.len() <= 1, "duplicate image metadata");
        let metadata = if let Some(mut entry) = embedded.pop() {
            entry.decompress_text_with_limit(16_384)?;
            let text = entry.get_text()?;
            ensure!(text.len() <= 16_384, "image metadata exceeds 16 KiB");
            let metadata: CaptureMetadata =
                serde_json::from_str(&text).context("invalid image metadata")?;
            let dimensions =
                |value: Option<png::PixelDimensions>| value.map(|p| (p.xppu, p.yppu, p.unit));
            ensure!(
                dimensions(metadata.resolution.pixel_dimensions()) == dimensions(physical),
                "PNG resolution conflicts with embedded provenance"
            );
            metadata
        } else {
            let mut metadata = CaptureMetadata::default();
            if let Some(png::PixelDimensions {
                xppu,
                yppu,
                unit: png::Unit::Meter,
            }) = physical
            {
                ensure!(xppu > 0 && yppu > 0, "PNG declares zero resolution");
                metadata.resolution = Resolution::Known {
                    x_ppi: f64::from(xppu) * 0.0254,
                    y_ppi: f64::from(yppu) * 0.0254,
                    provenance: ResolutionProvenance::PngPhysicalDimensions,
                };
            }
            metadata
        };
        let image = Image {
            width,
            height,
            pixels,
        };
        metadata.validate(&image)?;
        Ok(Self { image, metadata })
    }
}
