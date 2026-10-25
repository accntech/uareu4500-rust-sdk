// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional OpenJPEG lossless grayscale raw-codestream adapter.
//! JP2 containers, multiple tiles, color, signed/high-bit-depth and subsampled images are rejected.
//! Raw codestreams carry no physical scale; callers may preserve explicit metadata.
use crate::{
    frame::Image,
    metadata::{CaptureMetadata, CapturedImage, ProcessingStep},
    recognition::{Workspace, private_file, read_bounded},
};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_PIXELS: usize = 1024 * 1024;
#[derive(Clone, Debug)]
pub struct Jpeg2000Codec {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub timeout: Duration,
}
fn geometry(bytes: &[u8]) -> Result<(usize, usize)> {
    ensure!(
        bytes.len() >= 47
            && bytes.len() <= MAX_BYTES
            && bytes.starts_with(&[0xff, 0x4f, 0xff, 0x51])
            && bytes.ends_with(&[0xff, 0xd9]),
        "unsupported JPEG2000 raw codestream envelope"
    );
    ensure!(
        bytes[4..8] == [0, 41, 0, 0] && bytes[40..45] == [0, 1, 7, 1, 1],
        "JPEG2000 requires baseline one unsigned8-bit unsubsampled component"
    );
    let word = |i| u32::from_be_bytes(bytes[i..i + 4].try_into().expect("bounded SIZ field"));
    let (width, height) = (word(8) as usize, word(12) as usize);
    ensure!(
        width > 0
            && height > 0
            && width <= 4096
            && height <= 4096
            && width.checked_mul(height).is_some_and(|n| n <= MAX_PIXELS),
        "JPEG2000 geometry exceeds adapter limits"
    );
    ensure!(
        word(16) == 0
            && word(20) == 0
            && word(24) == width as u32
            && word(28) == height as u32
            && word(32) == 0
            && word(36) == 0,
        "unsupported JPEG2000 image/tile origin or extent"
    );
    Ok((width, height))
}
impl Jpeg2000Codec {
    fn run(&self, executable: &Path, args: &[OsString], workspace: &Path) -> Result<()> {
        ensure!(
            self.timeout > Duration::ZERO && self.timeout <= Duration::from_secs(300),
            "JPEG2000 codec deadline out of range"
        );
        ensure!(executable.is_file(), "OpenJPEG executable unavailable");
        let stderr = workspace.join("stderr.txt");
        let mut command = Command::new(fs::canonicalize(executable)?);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(private_file(&workspace.join("stdout.txt"))?)
            .stderr(private_file(&stderr)?);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Child(command.spawn()?);
        let start = Instant::now();
        loop {
            ensure!(start.elapsed() < self.timeout, "OpenJPEG codec timed out");
            let mut total = 0u64;
            let mut count = 0;
            for path in fs::read_dir(workspace)? {
                let n = path?.metadata()?.len();
                count += 1;
                total += n;
                ensure!(
                    n <= MAX_BYTES as u64 && total <= 8 * 1024 * 1024 && count <= 8,
                    "OpenJPEG output limit exceeded"
                );
            }
            if let Some(status) = child.0.try_wait()? {
                ensure!(
                    status.success(),
                    "OpenJPEG codec failed: {}",
                    String::from_utf8_lossy(&read_bounded(&stderr, 65536)?)
                );
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// Encode a lossless raw J2K codestream. No resolution is inferred or embedded.
    pub fn encode(&self, captured: &CapturedImage) -> Result<Vec<u8>> {
        captured.metadata.validate(&captured.image)?;
        let image = &captured.image;
        ensure!(
            image.width <= 4096 && image.height <= 4096 && image.pixels.len() <= MAX_PIXELS,
            "JPEG2000 image exceeds adapter limits"
        );
        let workspace = Workspace::new()?;
        let input = workspace.0.join("image.pgm");
        let output = workspace.0.join("image.j2k");
        let mut f = private_file(&input)?;
        write!(f, "P5\n{} {}\n255\n", image.width, image.height)?;
        f.write_all(&image.pixels)?;
        drop(f);
        let resolutions = (image.width.min(image.height).ilog2() + 1)
            .min(6)
            .to_string();
        self.run(
            &self.encoder,
            &[
                "-i".into(),
                input.into_os_string(),
                "-o".into(),
                output.clone().into_os_string(),
                "-r".into(),
                "1".into(),
                "-n".into(),
                resolutions.into(),
                "-threads".into(),
                "1".into(),
            ],
            &workspace.0,
        )?;
        let bytes = read_bounded(&output, MAX_BYTES)?;
        ensure!(
            geometry(&bytes)? == (image.width, image.height),
            "OpenJPEG encoded geometry mismatch"
        );
        Ok(bytes)
    }
    /// Decode unsigned 8-bit grayscale raw J2K. Physical scale remains unknown.
    pub fn decode(&self, bytes: &[u8]) -> Result<CapturedImage> {
        self.decode_with_metadata(bytes, CaptureMetadata::default())
    }
    /// Retain caller-supplied metadata after checking it against decoded geometry.
    /// The supplied resolution remains a declaration; J2K supplies no DPI evidence.
    pub fn decode_with_metadata(
        &self,
        bytes: &[u8],
        mut metadata: CaptureMetadata,
    ) -> Result<CapturedImage> {
        let (width, height) = geometry(bytes)?;
        let workspace = Workspace::new()?;
        let input = workspace.0.join("image.j2k");
        let output = workspace.0.join("image.pgm");
        private_file(&input)?.write_all(bytes)?;
        self.run(
            &self.decoder,
            &[
                "-i".into(),
                input.into_os_string(),
                "-o".into(),
                output.clone().into_os_string(),
                "-threads".into(),
                "1".into(),
            ],
            &workspace.0,
        )?;
        let pgm = read_bounded(&output, MAX_PIXELS + 4096)?;
        let pixels = parse_pgm(&pgm, width, height)?;
        let image = Image {
            width,
            height,
            pixels,
        };
        metadata.validate(&image)?;
        metadata
            .processing
            .push(ProcessingStep::Jpeg2000Decompression);
        Ok(CapturedImage { image, metadata })
    }
}
fn parse_pgm(bytes: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    let mut position = 0;
    fn token<'a>(bytes: &'a [u8], position: &mut usize) -> Result<&'a [u8]> {
        loop {
            while bytes.get(*position).is_some_and(u8::is_ascii_whitespace) {
                *position += 1;
            }
            if bytes.get(*position) == Some(&b'#') {
                while bytes.get(*position).is_some_and(|&b| b != b'\n') {
                    *position += 1;
                }
            } else {
                break;
            }
        }
        let start = *position;
        while bytes
            .get(*position)
            .is_some_and(|b| !b.is_ascii_whitespace())
        {
            *position += 1;
        }
        ensure!(*position > start, "truncated OpenJPEG PGM header");
        Ok(&bytes[start..*position])
    }
    ensure!(
        token(bytes, &mut position)? == b"P5",
        "OpenJPEG output must be binary grayscale PGM"
    );
    let mut number = || -> Result<usize> {
        std::str::from_utf8(token(bytes, &mut position)?)?
            .parse()
            .context("invalid OpenJPEG PGM number")
    };
    ensure!(
        number()? == width && number()? == height && number()? == 255,
        "OpenJPEG PGM geometry/depth mismatch"
    );
    let separator = *bytes
        .get(position)
        .context("missing OpenJPEG PGM separator")?;
    ensure!(
        separator.is_ascii_whitespace(),
        "missing OpenJPEG PGM separator"
    );
    position += 1;
    if separator == b'\r' && bytes.get(position) == Some(&b'\n') {
        position += 1;
    }
    ensure!(
        bytes.len() - position == width * height,
        "OpenJPEG PGM payload length mismatch"
    );
    Ok(bytes[position..].to_vec())
}
