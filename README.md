# UareU 4500 Rust SDK

Experimental Rust library and command-line tool for U.are.U 4500 USB image
capture on Windows, Linux, and macOS. Licensed **GPL-3.0-or-later**; see [LICENSE](LICENSE)
and [NOTICE](NOTICE) for the grant, attribution, and upstream license origin.

Targets USB **05ba:000a**, shared by 4000B/4500 readers. Device behavior and timing
still require physical-reader verification on each OS. Tests use deterministic,
non-biometric patterns; they are not hardware acceptance evidence. This tool
captures images, not enrollment templates, matches, or liveness assessments.

## Build and use

Requires stable Rust 1.85+ and a USB-connected reader. Project tooling also runs
in Rust through `cargo xtask`; Python is not required. The application does not
install a custom kernel driver. Windows requires the reader to be associated with
Microsoft's built-in WinUSB driver; see [Windows USB setup](#windows-usb-setup).

```sh
cargo build --locked --release
./target/release/uareu4500 list
./target/release/uareu4500 capture --output fingerprint.png --timeout 30
./target/release/uareu4500 license
```

To keep scanning without restarting the app:

```sh
./target/release/uareu4500 capture --continuous --output-dir captures
```

Place your finger to capture, lift it when prompted, then place it again for the
next image. Holding a finger on the sensor does not create repeated images.
Each capture receives a unique `fingerprint-<session>-<sequence>.png` filename;
existing files are never overwritten. The output directory defaults to `captures`.
Press Ctrl-C to stop and release the reader. Continuous mode requires a directory
rather than the single-capture `--output` file.

Place a finger after the prompt; Ctrl-C cancels. Output is a new 384 x 290,
8-bit grayscale PNG with Unix mode `0600`; existing files are not overwritten.
No biometric data is uploaded. Store and delete images appropriately.

`list --all` helps diagnose unexpected USB IDs. With multiple readers, specify
`capture --device INDEX`; indices can change after reconnection. `--no-rotate`
disables the default 180-degree rotation; polarity is normalized either way.
For a single capture, the deadline covers the capture session, not OS
open/configure/close operations. In continuous mode, `--timeout` bounds startup
and each image acquisition; waiting for finger placement or removal can remain
idle until Ctrl-C. Cleanup has an additional bounded allowance. Single captures
consume the reader; continuous mode keeps one reader open for the scan sequence.

### Windows USB setup

The `nusb` backend uses WinUSB on Windows. Building or copying the executable
does not assign this driver to the reader: each Windows PC needs an initial
driver setup with administrator approval. Installing WinUSB allows this SDK to
communicate with the reader; fingerprint capture remains in the Rust application.

1. Connect the reader and download [Zadig](https://zadig.akeo.ie/) from its
   official site. Verify that the executable's digital signature is valid and
   identifies **Akeo Consulting**, then run it as administrator.
2. Select the fingerprint reader. It may appear as **Biokey 200 Fingerprint
   Scanner**. Confirm the **USB ID is `05BA:000A`** before installing anything.
   If it is missing from the list, enable **Options > List All Devices**.
3. Select **WinUSB** as the target driver and click **Install Driver** (or
   **Replace Driver** if another driver is assigned). Replacing a manufacturer's
   driver may prevent its software from using the reader until that driver is
   restored. Do not select a keyboard, mouse, or USB hub.
4. Wait for installation to finish, then verify detection and start scanning:

   ```powershell
   cargo build --locked --release
   .\target\release\uareu4500.exe list
   .\target\release\uareu4500.exe capture --continuous --output-dir captures
   ```

Place a finger when prompted, then lift and replace it between captures. Press
Ctrl-C to stop. Once WinUSB is assigned, subsequent runs normally need no
administrator approval; moving the executable to a fresh PC still requires the
driver setup there.

If Device Manager shows the reader with **Code 28**, Windows has no driver
installed for it. The CLI may report no compatible reader even though Device
Manager shows `USB\VID_05BA&PID_000A`. Assign WinUSB using the steps above and
retry `list`; reconnect the reader if necessary. `list --all` alone cannot rule
out a connected reader whose Windows driver is missing or incompatible.

See Microsoft's [WinUSB installation documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/winusb-installation)
and [Code 28 explanation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/cm-prob-failed-install).

### Linux and macOS USB setup

On Linux, install the reader-scoped active-session rule, then reconnect:

```sh
sudo install -m 0644 udev/70-uareu4500.rules /etc/udev/rules.d/70-uareu4500.rules
sudo udevadm control --reload-rules
```

Headless services need a dedicated group/0660 rule instead of `uaccess`. Release
any reader interface held by fprintd or other software before direct capture.
On macOS, approve the USB accessory if prompted. The backend uses IOKit; no kext
or DriverKit extension is required. This does not integrate with Apple Touch ID.

## Image metadata and offline quality

Resolution is **unknown by default**. A reader's USB ID, product name, or nominal
datasheet resolution does not establish the effective PPI of this decoder's
output. Neither capture nor quality inspection assumes 500 or 512 PPI.

The CLI now saves processing and descriptor-coverage metadata in PNG iTXt under
`uareu4500.metadata.v1`. Known resolution is also written to the PNG `pHYs` chunk
as pixels per metre, rounded to the nearest integer. Unknown resolution omits
`pHYs`. Metadata records its source, not a claim of independently verified
hardware calibration. Metadata edited outside this application remains an
untrusted declaration; imports reject conflicting metadata and `pHYs` values.

Inspect a previously captured image without opening USB hardware:

```powershell
.\target\release\uareu4500.exe quality --input fingerprint.png
```

The JSON report contains resolution provenance, known processing, and image
diagnostics: minimum/maximum intensity, mean, population standard deviation,
fractions at 0 and 255, and known padding coverage. When descriptor coverage is
available, diagnostics exclude only the known padding. That region is not finger
segmentation. Older PNGs have unknown padding and processing history. These
diagnostics do not establish biometric quality, identity, or liveness.

`capture --ppi VALUE` explicitly declares square-pixel output resolution and
records `caller_declared` provenance. `quality --ppi VALUE` overrides the input's
resolution for that report only, also as a caller declaration. Both options leave
pixels unchanged: **declaring PPI does not resample an image**. Use a declaration
only when the actual output resolution has been established independently.

### Optional NFIQ 2

Install a reference [NIST NFIQ 2 v2.3.0 CLI](https://github.com/usnistgov/NFIQ2/releases/tag/v2.3.0)
separately. No quality engine is downloaded, bundled, or required for capture.
For an image already established as 500 PPI on both axes:

```powershell
.\target\release\uareu4500.exe quality --input known-500ppi.png --nfiq2 C:\NFIQ2\bin\nfiq2.exe
```

Embedded-model builds identify the model through the executable SHA-256. For a
build using an external model, add `--nfiq2-model C:\NFIQ2\nist_plain_tir-ink.txt`.
The adapter passes this information file to the engine and records SHA-256 hashes
of both the information file and the referenced model data. Supplying a model
file to an embedded-model build is rejected because that build ignores the option.

The adapter checks the reported version, uses a private temporary grayscale PNG,
and removes its workspace on normal return, including evaluation failures. On
Unix the temporary directory is mode `0700` and files are `0600`; Windows uses
the current user's temporary-directory ACLs. Child processes run without a shell,
with a per-process deadline (`--engine-timeout`, default 10 seconds, maximum 300)
and bounded output reads; excessive output terminates evaluation. Abrupt process
termination can leave temporary files, so do not treat cleanup as secure erasure.

Unknown resolution and non-500-PPI input produce `unsupported_input`. Resampling
is not implemented. Only PNG encoding round-off (at most about 0.013 PPI per axis)
is tolerated. Known descriptor padding is cropped for evaluation and the crop is
reported. The engine may reject images further after its own whitespace cropping.
Unexpected engine quantization/resampling is rejected rather than silently accepted.

Without an engine the outcome is `not_evaluated`. Other outcomes are
`engine_unavailable`, `evaluation_failed`, or `scored`. The latter contains an
NFIQ 2 score in `0..100` (higher is better), engine/model identity, and upstream
actionable measures. A failure is never represented by a score of zero. Quality
evaluation failures retain diagnostics in JSON and return a nonzero CLI exit
status; successful scoring and diagnostics-only inspection exit successfully.
Malformed input fails before a report is produced. No automatic acceptance
threshold or capture rejection is applied. NFIQ 2 is not the legacy NFIQ 1 score
provided by the U.are.U 3.0 SDK; there is no score conversion or SDK-parity claim.

The [pinned CLI contract](https://github.com/usnistgov/NFIQ2/blob/v2.3.0/NFIQ2/NFIQ2Algorithm/doc/nfiq2_tool.1.md)
defines input restrictions. Tests use a process fixture to exercise parsing,
version/model handling, failures, output limits, and deadlines; that fixture does
not implement biometric scoring. NIST's separately licensed conformance images
are not included. Real-engine conformance and recognition accuracy require
independent testing.

An opt-in integration test compares the real CLI's PGM score with the adapter's
PNG score on identical synthetic pixels. Supply the executable, its external
model information (omit this variable for embedded-model builds), and a
synthetic 500-PPI P5 PGM such as the upstream `examples/images/SFinGe_Test01.pgm`:

```powershell
$env:UAREU_NFIQ2_EXE = 'C:\NFIQ2\bin\nfiq2.exe'
$env:UAREU_NFIQ2_MODEL = 'C:\NFIQ2\nist_plain_tir-ink.txt'
$env:UAREU_NFIQ2_TEST_PGM = 'C:\test-data\SFinGe_Test01.pgm'
cargo test --locked --test nfiq2_reference -- --ignored --nocapture
```

This adapter check is separate from the NIST conformance suite and does not
establish recognition accuracy or the resolution of physical-reader captures.

## Rust library

```rust,no_run
use std::{sync::atomic::AtomicBool, time::Duration};
use uareu4500::{protocol::CaptureOptions, usb::{readers, Reader}};

fn main() -> anyhow::Result<()> {
    let devices = readers()?;
    let info = devices.first().ok_or_else(|| anyhow::anyhow!("reader not connected"))?;
    let image = Reader::open(info)?.capture(
        CaptureOptions { timeout: Duration::from_secs(30), rotate180: true },
        &AtomicBool::new(false),
        |event| eprintln!("{event:?}"),
    )?;
    image.save_png("fingerprint.png")?;
    Ok(())
}
```

`protocol::Transport` supports simulated USB tests. `frame::decode_frame` supports
offline wire-frame analysis. The format and inferred behavior are documented in
[the protocol specification](docs/PROTOCOL_FACTS.md). Legacy scrambling is not a
modern secure transport guarantee. Hardware acceptance must cover repeated
capture, cancellation, unplug/reconnect, image orientation, and timing.

For streaming from a library application, use `Reader::capture_continuous` with
an image callback returning `anyhow::Result<std::ops::ControlFlow<()>>`. Return
`Continue(())` for another scan or `Break(())` to finish. The SDK waits for actual
finger removal between scans. Callback errors stop the session and propagate to
the caller after cleanup; cancellation stops continuous scanning gracefully.

The existing `Image`, `decode_frame`, `Reader::capture`, and
`Reader::capture_continuous` APIs retain their signatures and pixel output.
Metadata-bearing variants are `frame::decode_frame_with_metadata`,
`Reader::capture_with_metadata`, and `Reader::capture_continuous_with_metadata`;
the same variants are available in `protocol` for simulated transports. They
return `metadata::CapturedImage`, which wraps the original `Image` with
`CaptureMetadata`. Rotation transforms descriptor coverage along with pixels.
No resolution is inferred by these methods. `Image::save_png` retains its legacy
behavior; use `CapturedImage::save_png` to preserve metadata.

```rust,no_run
use uareu4500::{metadata::CapturedImage, quality};

fn inspect() -> anyhow::Result<()> {
    let captured = CapturedImage::load_png("fingerprint.png")?;
    let report = quality::assess(&captured, None)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
```

`quality::Nfiq2` supplies the optional engine configuration. Keep this offline
assessment separate from capture callbacks when cancellation latency matters.

## Tests and fixtures

```sh
cargo test --locked --all --workspace
cargo clippy --locked --all-targets --workspace -- -D warnings
cargo fmt --all -- --check
cargo xtask fixtures --check
```

The workspace's Rust `xtask` tool generates the four fixture files from chosen
sensor patterns and an independent encoder in `xtask/src/fixtures.rs`.
Expected pixels originate before wire encoding. No upstream recording/image is
included. Regenerate with the same command without `--check`.

## Linux/macOS release packages

Use a suitable native builder and an ordinary unmodified Rust toolchain. Install
the target and Rust documentation for the matching runtime copyright report:

```sh
rustup component add rust-docs
rustup target add aarch64-apple-darwin
cargo xtask package --target aarch64-apple-darwin
```

Other targets are `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, and
`aarch64-unknown-linux-gnu`. A Rust target alone does not supply a cross-linker or
platform SDK. Native Linux and Intel/Apple silicon macOS builders are configured
in `.github/workflows/release-packages.yml`.

The packager freezes the required project inputs, vendors the exact locked Cargo
dependencies, builds offline from that snapshot, and embeds the matching snapshot
under `source/` in the binary archive. It includes source, tests/fixtures, build
and packaging scripts, Cargo manifests/lockfile, and the offline vendor config.
`LICENSE`, `NOTICE`, dependency notices, and the matching Rust standard-library
notice report accompany the executable. Build/source hashes and toolchain details
are recorded in the archive; `.sha256` files verify the archive bytes.

To rebuild after extracting a package, enter `source/` and use the recorded Rust
version plus the platform SDK/system libraries:

```sh
./REBUILD.sh
```

The standard unmodified compiler/toolchain and qualifying system libraries are
not bundled as source. Custom runtime/toolchain changes or additional non-system
native components require a separate source-compliance assessment. Signing and
notarization are not performed. Linux GNU builds depend on the builder's system
libraries; they are not universal static binaries.

Archives go in ignored `dist/`. Existing archives are not overwritten; use
`--output-dir` for another build. Local packaging uploads nothing. CI uploads
whole source-inclusive archives as Actions artifacts on manual runs or `v*` tags;
it does not publish a GitHub Release. Distribute the whole archive with its source,
not a bare executable detached from its source-access information.

## Dependency notices

`cargo xtask notices` writes generated reports under ignored
`target/license-notices/`; `--check` verifies freshly generated outputs. Release
packaging creates target-filtered reports automatically. Reviewed license hashes
and upstream references are embedded in `xtask/src/policy.rs`. The texts in
`licenses/supplements/` provide canonical terms and exact upstream supplements for
crates that omit notices. Available copyright/licensing statements are preserved without
inventing ownership; Cargo author metadata is labelled separately. Dependencies
retain their own grants. Their source and notices are included in release packages.

GPL permits commercial use, but distributing applications that incorporate this
library entails GPL obligations for the combined covered work. The full grant,
upstream conversion scope, changes, and prior-license limitations are in NOTICE.
