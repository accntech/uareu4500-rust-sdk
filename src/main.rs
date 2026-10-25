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
    protocol::{CaptureEvent, CaptureOptions},
    usb::{Reader, readers},
};

#[derive(Parser)]
#[command(
    version,
    about = "UareU 4500 Rust SDK: experimental image capture for Linux and macOS"
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
    /// Wait for a finger and save one 384 x 290 grayscale PNG.
    Capture {
        /// New output file; existing files are never overwritten.
        #[arg(short, long)]
        output: PathBuf,
        /// Overall capture timeout in seconds (1..=3600), excluding USB open/close.
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
        /// Reader index shown by list. Required when multiple readers are connected.
        #[arg(long)]
        device: Option<usize>,
        /// Keep sensor orientation instead of the default 180-degree rotation.
        #[arg(long)]
        no_rotate: bool,
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
        Commands::Capture {
            output,
            timeout,
            device,
            no_rotate,
        } => {
            ensure!(
                !output.exists(),
                "output already exists: {}",
                output.display()
            );
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
            let image = reader.capture(
                CaptureOptions {
                    timeout: Duration::from_secs(timeout),
                    rotate180: !no_rotate,
                },
                &cancelled,
                |event| {
                    eprintln!(
                        "{}",
                        match event {
                            CaptureEvent::Initializing => "Initializing reader...",
                            CaptureEvent::WaitingForFinger =>
                                "Place your finger on the reader (Ctrl-C to cancel).",
                            CaptureEvent::Capturing => "Capturing fingerprint...",
                        }
                    );
                },
            )?;
            image.save_png(&output)?;
            println!(
                "Saved {} x {} grayscale image to {}",
                image.width,
                image.height,
                output.display()
            );
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
