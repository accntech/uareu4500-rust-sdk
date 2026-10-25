// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use nusb::MaybeFuture;
use std::{
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use uareu4500::{
    metadata::{CapturedImage, Resolution},
    protocol::{CaptureEvent, CaptureOptions},
    quality::{self, Nfiq2, QualityOutcome},
    usb::{Reader, readers},
};

#[derive(Parser)]
#[command(
    version,
    about = "UareU 4500 Rust SDK: experimental fingerprint image capture and diagnostics"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Show license, attribution, and warranty terms.
    License,
    /// List compatible readers, without claiming or initializing them.
    List {
        /// Also list other USB devices for detection troubleshooting.
        #[arg(long)]
        all: bool,
    },
    /// Inspect an 8-bit grayscale PNG offline; optionally run NFIQ 2 v2.3.0.
    Quality {
        #[arg(long)]
        input: PathBuf,
        /// Explicit square-pixel resolution declaration; does not resample pixels.
        #[arg(long)]
        ppi: Option<f64>,
        /// File path of an externally installed NFIQ 2 v2.3.0 executable.
        #[arg(long)]
        nfiq2: Option<PathBuf>,
        /// Model information file passed to NFIQ 2; contains a Path entry.
        #[arg(long, requires = "nfiq2")]
        nfiq2_model: Option<PathBuf>,
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=300))]
        engine_timeout: u64,
    },
    /// Save a fingerprint image, or keep scanning after each finger removal.
    Capture {
        /// New output file; existing files are never overwritten.
        #[arg(
            short,
            long,
            required_unless_present = "continuous",
            conflicts_with = "continuous"
        )]
        output: Option<PathBuf>,
        /// Keep scanning until Ctrl-C; lift and replace your finger between images.
        #[arg(long)]
        continuous: bool,
        /// Directory for unique PNGs in continuous mode (default: captures).
        #[arg(long, requires = "continuous")]
        output_dir: Option<PathBuf>,
        /// Timeout in seconds: entire single scan; startup/acquisition in continuous mode.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
        /// Reader index shown by list. Required when multiple readers are connected.
        #[arg(long)]
        device: Option<usize>,
        /// Keep sensor orientation instead of the default 180-degree rotation.
        #[arg(long)]
        no_rotate: bool,
        /// Explicit output PPI declaration; unknown by default. Does not resample.
        #[arg(long)]
        ppi: Option<f64>,
    },
}
fn run() -> Result<()> {
    match Cli::parse().command {
        Commands::License => {
            println!(
                "{}\n{}",
                include_str!("../NOTICE"),
                include_str!("../LICENSE")
            );
        }
        Commands::List { all } => {
            if all {
                for device in nusb::list_devices()
                    .wait()
                    .context("cannot enumerate USB devices")?
                {
                    println!(
                        "USB {:04x}:{:04x} rev={:04x} {}",
                        device.vendor_id(),
                        device.product_id(),
                        device.device_version(),
                        device.product_string().unwrap_or("(unnamed)")
                    );
                }
                println!();
            }
            let devices = readers()?;
            if devices.is_empty() {
                println!(
                    "No compatible reader found (expected USB 05ba:000a). Connect a U.are.U 4500 and run list again."
                );
            }
            for (index, device) in devices.iter().enumerate() {
                println!(
                    "[{index}] {:04x}:{:04x} rev={:04x} {}",
                    device.vendor_id(),
                    device.product_id(),
                    device.device_version(),
                    device.product_string().unwrap_or("U.are.U 4000B/4500")
                );
            }
        }
        Commands::Quality {
            input,
            ppi,
            nfiq2,
            nfiq2_model,
            engine_timeout,
        } => {
            let mut captured = CapturedImage::load_png(input)?;
            if let Some(ppi) = ppi {
                captured.metadata.resolution = Resolution::declared(ppi, ppi)?;
            }
            let engine = nfiq2.map(|executable| Nfiq2 {
                executable,
                model_info: nfiq2_model,
                timeout: Duration::from_secs(engine_timeout),
            });
            let report = quality::assess(&captured, engine.as_ref())?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
            println!();
            ensure!(
                matches!(
                    report.quality,
                    QualityOutcome::NotEvaluated | QualityOutcome::Scored { .. }
                ),
                "quality evaluation did not produce a score; see the JSON status"
            );
        }
        Commands::Capture {
            output,
            continuous,
            output_dir,
            timeout,
            device,
            no_rotate,
            ppi,
        } => {
            let declared_ppi = ppi.map(|ppi| Resolution::declared(ppi, ppi)).transpose()?;
            ensure!(
                continuous || output_dir.is_none(),
                "--output-dir requires --continuous"
            );
            if let Some(output) = &output {
                ensure!(
                    !output.exists(),
                    "output already exists: {}",
                    output.display()
                );
            }
            let mut files = if continuous {
                Some(ContinuousFiles::new(
                    output_dir.unwrap_or_else(|| PathBuf::from("captures")),
                )?)
            } else {
                None
            };
            let devices = readers()?;
            ensure!(
                !devices.is_empty(),
                "no U.are.U 4500 found (expected USB 05ba:000a); run list --all to inspect connected devices"
            );
            let index = match device {
                Some(index) => index,
                None if devices.len() == 1 => 0,
                None => bail!("multiple readers found; use list, then capture --device INDEX"),
            };
            let info = devices
                .get(index)
                .context("reader index out of range; run list")?;
            let cancelled = Arc::new(AtomicBool::new(false));
            let signal = Arc::clone(&cancelled);
            ctrlc::set_handler(move || {
                signal.store(true, std::sync::atomic::Ordering::Relaxed);
            })
            .context("cannot install cancellation handler")?;
            let reader = Reader::open(info)?;
            let options = CaptureOptions {
                timeout: Duration::from_secs(timeout),
                rotate180: !no_rotate,
            };
            if let Some(files) = &mut files {
                reader.capture_continuous_with_metadata(
                    options,
                    &cancelled,
                    report_event,
                    |mut captured| {
                        if let Some(resolution) = &declared_ppi {
                            captured.metadata.resolution = resolution.clone();
                        }
                        let path = files.save(&captured)?;
                        report_saved_image(&mut std::io::stdout().lock(), &captured.image, &path)?;
                        Ok(std::ops::ControlFlow::Continue(()))
                    },
                )?;
                println!("Continuous scanning stopped.");
            } else {
                let mut captured =
                    reader.capture_with_metadata(options, &cancelled, report_event)?;
                if let Some(resolution) = declared_ppi {
                    captured.metadata.resolution = resolution;
                }
                let output = output.context("single capture requires --output")?;
                captured.save_png(&output)?;
                println!(
                    "Saved {} x {} grayscale image to {}",
                    captured.image.width,
                    captured.image.height,
                    output.display()
                );
            }
        }
    }
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

struct ContinuousFiles {
    directory: PathBuf,
    prefix: String,
    next: u64,
}
impl ContinuousFiles {
    fn new(directory: PathBuf) -> Result<Self> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .with_context(|| format!("cannot create output directory {}", directory.display()))?;
        let mut nonce = [0u8; 8];
        getrandom::fill(&mut nonce).context("cannot generate capture filename")?;
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        Ok(Self {
            directory,
            prefix: format!("fingerprint-{seconds}-{:016x}", u64::from_le_bytes(nonce)),
            next: 1,
        })
    }
    fn save(&mut self, image: &CapturedImage) -> Result<PathBuf> {
        for _ in 0..1000 {
            let path = self
                .directory
                .join(format!("{}-{:06}.png", self.prefix, self.next));
            self.next = self
                .next
                .checked_add(1)
                .context("capture filename counter exhausted")?;
            match image.save_png(&path) {
                Ok(()) => return Ok(path),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists) =>
                {
                    continue;
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("cannot save capture {}", path.display()));
                }
            }
        }
        bail!(
            "too many existing capture filenames in {}",
            self.directory.display()
        )
    }
}

fn report_event(event: CaptureEvent) {
    use std::io::Write;
    // Status output is best effort: a closed log pipe must not panic past cleanup.
    let _ = writeln!(
        std::io::stderr().lock(),
        "{}",
        match event {
            CaptureEvent::Initializing => "Initializing reader...",
            CaptureEvent::WaitingForFinger => "Place your finger on the reader (Ctrl-C to cancel).",
            CaptureEvent::WaitingForFingerRemoval =>
                "Lift your finger before the next scan (Ctrl-C to stop).",
            CaptureEvent::Capturing => "Capturing fingerprint...",
        }
    );
}

fn report_saved_image(
    output: &mut impl std::io::Write,
    image: &uareu4500::frame::Image,
    path: &std::path::Path,
) -> Result<()> {
    writeln!(
        output,
        "Saved {} x {} grayscale image to {}",
        image.width,
        image.height,
        path.display()
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let mut nonce = [0u8; 16];
            getrandom::fill(&mut nonce).unwrap();
            let path = std::env::temp_dir()
                .join(format!("uareu-images-{:032x}", u128::from_le_bytes(nonce)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn image() -> uareu4500::frame::Image {
        uareu4500::frame::Image {
            width: 2,
            height: 1,
            pixels: vec![0, 255],
        }
    }
    #[test]
    fn continuous_files_create_unique_private_decodable_images() {
        let temp = TestDirectory::new();
        let directory = temp.0.join("new");
        let mut files = ContinuousFiles::new(directory.clone()).unwrap();
        let first = files.save(&CapturedImage::new(image())).unwrap();
        let second = files.save(&CapturedImage::new(image())).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(directory.as_path()));
        for path in [first, second] {
            let mut reader =
                png::Decoder::new(std::io::BufReader::new(fs::File::open(&path).unwrap()))
                    .read_info()
                    .unwrap();
            let mut data = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut data).unwrap();
            assert_eq!((info.width, info.height), (2, 1));
            assert_eq!(&data[..info.buffer_size()], &[0, 255]);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
    #[test]
    fn continuous_files_preserve_existing_names_and_skip_collision() {
        let temp = TestDirectory::new();
        let mut files = ContinuousFiles::new(temp.0.clone()).unwrap();
        let existing = temp.0.join(format!("{}-000001.png", files.prefix));
        fs::write(&existing, b"keep me").unwrap();
        let saved = files.save(&CapturedImage::new(image())).unwrap();
        assert_ne!(saved, existing);
        assert_eq!(fs::read(existing).unwrap(), b"keep me");
        assert!(saved.is_file());
    }
    #[test]
    fn continuous_files_reject_a_file_as_output_directory() {
        let temp = TestDirectory::new();
        let path = temp.0.join("file");
        fs::write(&path, b"keep me").unwrap();
        assert!(ContinuousFiles::new(path.clone()).is_err());
        assert_eq!(fs::read(path).unwrap(), b"keep me");
    }

    #[test]
    fn continuous_output_failure_is_returned_for_reader_cleanup() {
        struct BrokenOutput;
        impl std::io::Write for BrokenOutput {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "output closed",
                ))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let error = report_saved_image(
            &mut BrokenOutput,
            &image(),
            std::path::Path::new("image.png"),
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }
}
