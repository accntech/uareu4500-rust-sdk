# UareU Rust SDK

Experimental Rust library, CLI, and C ABI for U.are.U 4000B/4500 USB fingerprint
capture on Windows, Linux, and macOS. Supports USB **05ba:000a**, image metadata,
optional local quality/recognition engines, and bounded image/template interchange.
Licensed **GPL-3.0-or-later**; see [LICENSE](LICENSE) and [NOTICE](NOTICE).

Physical-reader behavior still needs verification on each OS. Capture resolution
is **unknown by default**; successful USB capture does not establish pixel scale,
biometric accuracy, liveness, or vendor SDK compatibility.

## Build and capture

Requires stable Rust 1.85+ and OS USB access configured below. Run commands from
the repository root. The crate and CLI are named `uareu`; C symbols use `uareu_*`.

```sh
cargo build --locked --release
./target/release/uareu list
./target/release/uareu capture --output fingerprint.png --timeout 30
./target/release/uareu capture --continuous --output-dir captures
```

On Windows, use `.\target\release\uareu.exe`. Place a finger when prompted, lift
between captures, and press Ctrl-C to stop. Output is 384 × 290, 8-bit grayscale
PNG; existing files are never overwritten. Continuous captures use unique
filenames in `captures` by default. Capture does not upload biometric data.

Useful options:

- `list --all`: inspect unexpected USB IDs.
- `capture --device INDEX`: select a reader; indices may change after reconnecting.
- `--no-rotate`: disable the default 180-degree rotation.
- `--timeout`: bound a single capture, or startup and each acquisition in continuous
  mode. Continuous finger placement/removal waits remain idle until cancellation;
  OS open/close and bounded cleanup are outside the acquisition deadline.

### Windows USB setup

Each Windows PC needs the reader associated with Microsoft's built-in **WinUSB**
driver. Building or copying this SDK does not install it.

1. Connect the reader. Download [Zadig](https://zadig.akeo.ie/) from its official
   site, verify its **Akeo Consulting** signature, and run it as administrator.
2. Select the reader, sometimes listed as **Biokey 200 Fingerprint Scanner**.
   Confirm **USB ID `05BA:000A`**. Enable **Options > List All Devices** if needed.
3. Select **WinUSB**, then **Install Driver** or **Replace Driver**. Replacing the
   manufacturer's driver may prevent its software from using the reader until
   restored. Do not select a keyboard, mouse, or USB hub.
4. Run `uareu.exe list`, then capture. Subsequent runs normally need no elevation.

Device Manager **Code 28** means no driver is installed. A reader can appear as
`USB\VID_05BA&PID_000A` there while the CLI cannot access it. Assign WinUSB,
reconnect if needed, and retry. See Microsoft's [WinUSB installation guide](https://learn.microsoft.com/en-us/windows-hardware/drivers/usbcon/winusb-installation)
and [Code 28 explanation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/cm-prob-failed-install).

### Linux and macOS USB setup

On Linux, install the reader-scoped active-session rule, then reconnect:

```sh
sudo install -m 0644 udev/70-uareu.rules /etc/udev/rules.d/70-uareu.rules
sudo udevadm control --reload-rules
```

Headless services need a dedicated group/0660 rule instead of `uaccess`. Release
interfaces held by fprintd or other software. On macOS, approve the USB accessory
if prompted; no kext or DriverKit extension is required. This SDK does not
integrate with Touch ID.

## Rust library

To use the SDK without CLI argument parsing and signal-handler dependencies:

```toml
[dependencies]
uareu = { version = "0.1.0", default-features = false }
```

The default `cli` feature preserves ordinary CLI builds. For a library-only build
from this checkout, use `cargo build --locked --lib --no-default-features`.

```rust,no_run
use std::{sync::atomic::AtomicBool, time::Duration};
use uareu::{protocol::CaptureOptions, usb::{readers, Reader}};

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

Use `capture_with_metadata` and `CapturedImage::save_png` to preserve processing
and resolution provenance. `capture_continuous` waits for removal between images;
its callback returns `ControlFlow::Continue(())` or `Break(())`. Callback errors
propagate after cleanup, while cancellation ends continuous scanning gracefully.
`capture_next` borrows and retains the reader for repeated operations.

`Reader::status_monitor()` provides cached lifecycle snapshots. Cached `idle`
does not assert physical readiness, and queries do not themselves poll USB.
Capabilities distinguish implemented SDK operations from unknown device features.
Rust errors retain typed `CaptureError` kind/stage and secondary cleanup details.
See the [protocol specification](fixtures/docs/PROTOCOL_FACTS.md) for wire-format
constraints and `protocol::Transport` for simulated transports.

## Windows DLL and C/C#

Build on x64 Windows with the `x86_64-pc-windows-msvc` toolchain and Visual Studio
C++ build tools. `cargo build --locked --release` produces `uareu.dll` and
`uareu.dll.lib` under `target/release/`. Deployment also needs WinUSB and the x64
Microsoft Visual C++ runtime (`VCRUNTIME140.dll`). No DigitalPersona runtime is loaded.

The ABI v1 contract is [include/uareu.h](include/uareu.h). Important ownership rules:

- Free each opaque handle with its matching API. Only one operation may lease a reader.
- Probe buffer sizes and retry `BUFFER_TOO_SMALL`; status JSON may change between calls.
- Retrieve thread-local errors on the failing thread. `uareu_last_error_json`
  supplies structured details; successful ordinary calls clear the previous error.
- Run blocking capture on a worker and cancel from another thread. Close requests
  cancellation; wait for capture/task cleanup before unloading the library.
- Phase/image callbacks run synchronously; completion callbacks run on the capture
  worker. Foreign callbacks must not throw or unwind across the ABI.
- Use `uareu_reader_open_ex` with `UAREU_OPEN_PERSISTENT` to retain the interface
  between captures. The original open mode releases and reopens it.

Build the .NET 10 [C# example](examples/csharp/UareuExample.csproj):

```powershell
dotnet build examples/csharp/UareuExample.csproj -c Release
$env:UAREU_DLL_PATH = (Resolve-Path target/release/uareu.dll).Path
dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --list
dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --capture fingerprint.png
dotnet examples/csharp/bin/Release/net10.0/UareuExample.dll --inspect fingerprint.png
```

See [advanced SDK usage](examples/engine/README.md) for async capture, recognition,
interchange, and helper setup. The SDK defines its own ABI; it does not implement
the proprietary U.are.U binary interface.

## Image metadata and quality

```sh
./target/release/uareu quality --input fingerprint.png
```

This inspects an image without USB access. Reports include resolution provenance,
processing history, intensity/contrast/clipping diagnostics, and known descriptor
padding. Diagnostics do not establish fingerprint validity or liveness.

Metadata is stored in PNG iTXt under `uareu.metadata.v1`; known scale also uses
`pHYs`. The legacy `uareu4500.metadata.v1` key remains readable. Conflicting scale
metadata or both keys are rejected. `capture --ppi VALUE` records a caller
resolution declaration; `quality --ppi VALUE` overrides it for the report only.
**Declaring PPI does not resample pixels.** Use a value established independently
for the decoder's actual output.

### Optional NFIQ 2

Install [NIST NFIQ 2 v2.3.0](https://github.com/usnistgov/NFIQ2/releases/tag/v2.3.0)
separately. For an image already established as 500 PPI on both axes:

```powershell
.\target\release\uareu.exe quality --input known-500ppi.png --nfiq2 C:\NFIQ2\bin\nfiq2.exe
```

External-model builds also need `--nfiq2-model C:\NFIQ2\nist_plain_tir-ink.txt`;
embedded-model builds reject that option. `--engine-timeout` defaults to 10 seconds
and accepts up to 300. Helpers run locally with bounded output and temporary
files; normal cleanup is not secure erasure.

Unknown/non-500-PPI input is unsupported; resampling is not provided. Scored
results contain `0..100` (higher is better) plus engine/model identity. Without
an engine, the result is `not_evaluated`; evaluation failures remain separate
outcomes and return a nonzero CLI status. NFIQ 1 is a separate legacy `1..5`
(lower is better) scale. No automatic acceptance threshold or score conversion is applied.

## Tests and validation

```sh
cargo test --locked --workspace --all-targets
cargo test --locked --workspace --doc
cargo clippy --locked --all-targets --workspace -- -D warnings
cargo fmt --all -- --check
cargo xtask fixtures --check
```

Fixtures are deterministic, non-biometric patterns generated by `xtask`; remove
`--check` to regenerate them. Tests using real external engines are opt-in and
list their required environment variables in `tests/*_reference.rs`.

On Windows, `./scripts/test-windows-dll.ps1` checks the C/C# boundary without a
reader. Add `-HardwareCancellation` for connected-reader lifecycle checks without
placing a finger. Synthetic tests do not establish hardware behavior, matching
accuracy, pixel scale, or standards certification. The [validation index](docs/validation/README.md)
and [implementation gaps](docs/IMPLEMENTATION_GAPS.md) track those separately.

## Linux/macOS release packages

Use a native builder with the platform SDK/system libraries and matching Rust
documentation. For example, on Apple silicon macOS:

```sh
rustup component add rust-docs
rustup target add aarch64-apple-darwin
cargo xtask package --target aarch64-apple-darwin
```

Also supported: `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, and
`aarch64-unknown-linux-gnu`. Installing a Rust target alone does not supply a cross-linker.

The packager creates an SDK/CLI and `xtask` source workspace, excluding the desktop
app. It reduces the snapshot lockfile without changing retained package identities,
vendors dependencies, and builds offline from that snapshot. Archives include
matching source, tests/fixtures, notices, source/build hashes, and toolchain details.
The checkout's workspace is unchanged. After extraction, enter `source/` and run
`./REBUILD.sh` with the recorded toolchain and platform dependencies.

Archives and checksums go in ignored `dist/`; existing outputs are not overwritten.
Use `--output-dir` for another destination. Packages are unsigned, Linux builds
are not universal static binaries, and standard toolchains/system libraries are
not bundled. Distribute the complete source-inclusive archive. CI uploads archives
as Actions artifacts on manual runs or `v*` tags, without publishing a GitHub Release.

`cargo xtask notices` generates dependency reports under `target/license-notices/`;
`--check` verifies them. Release packaging creates target-filtered reports. See
[NOTICE](NOTICE) for licensing scope and `licenses/supplements/` for pinned terms.

## Desktop app and optional features

[Fingerprint Studio](apps/fingerprint-studio/README.md) provides capture/preview
and optional continuous enrollment/verification: `cargo run -p fingerprint-studio`.
Matching requires the local SourceAFIS helper and an explicit pixel-scale declaration.

The SDK also supports bounded ISO/ANSI image/minutiae subsets, opaque extensions,
external NBIS WSQ/OpenJPEG adapters, incremental enrollment, async completion,
status-change waits, and explicit-reference flat-field correction. See
[advanced usage and limits](examples/engine/README.md) and the
[implementation gaps](docs/IMPLEMENTATION_GAPS.md) for contracts and validation.
