# UareU 4500 Rust SDK

Experimental Rust library and command-line tool for U.are.U 4500 USB image
capture on Linux and macOS. Licensed **GPL-3.0-or-later**; see [LICENSE](LICENSE)
and [NOTICE](NOTICE) for the grant, attribution, and upstream license origin.

Targets USB **05ba:000a**, shared by 4000B/4500 readers. Device behavior and timing
still require physical-reader verification on each OS. Tests use deterministic,
non-biometric patterns; they are not hardware acceptance evidence. This tool
captures images, not enrollment templates, matches, or liveness assessments.

## Build and use

Requires stable Rust 1.85+ and a USB-connected reader. Project tooling also runs
in Rust through `cargo xtask`; Python is not required. No Windows SDK or custom
kernel driver is installed.

```sh
cargo build --locked --release
./target/release/uareu4500 list
./target/release/uareu4500 capture --output fingerprint.png --timeout 30
./target/release/uareu4500 license
```

Place a finger after the prompt; Ctrl-C cancels. Output is a new 384 x 290,
8-bit grayscale PNG with Unix mode `0600`; existing files are not overwritten.
No biometric data is uploaded. Store and delete images appropriately.

`list --all` helps diagnose unexpected USB IDs. With multiple readers, specify
`capture --device INDEX`; indices can change after reconnection. `--no-rotate`
disables the default 180-degree rotation; polarity is normalized either way.
The deadline covers the capture session, not OS open/configure/close operations.
Cleanup has an additional bounded allowance. Open a fresh reader for each capture.

On Linux, install the reader-scoped active-session rule, then reconnect:

```sh
sudo install -m 0644 udev/70-uareu4500.rules /etc/udev/rules.d/70-uareu4500.rules
sudo udevadm control --reload-rules
```

Headless services need a dedicated group/0660 rule instead of `uaccess`. Release
any reader interface held by fprintd or other software before direct capture.
On macOS, approve the USB accessory if prompted. The backend uses IOKit; no kext
or DriverKit extension is required. This does not integrate with Apple Touch ID.

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
