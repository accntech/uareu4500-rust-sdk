# Fingerprint Studio

Native GPUI desktop app for USB fingerprint capture, grayscale preview, and
optional local three-impression enrollment and continuous verification.
Reader/helper work runs on a worker thread. Closing cancels capture and waits
for cleanup; a helper already running may finish within its deadline before exit.

## Run

Run from the repository root:

```sh
cargo run -p fingerprint-studio
```

On macOS, build and run a named app bundle:

```sh
./scripts/run-fingerprint-studio-macos.sh
```

Connect a U.are.U 4000B/4500 (`05ba:000a`) and choose **Refresh**. The app uses the
first discovered reader; disconnect others to select a particular device. Capture
needs no recognition helper. Follow the [OS USB setup](../../README.md#windows-usb-setup)
for drivers and permissions.

## Configure matching

Build the optional .NET 10 SourceAFIS helper:

```sh
dotnet restore examples/engine/UareuEngine.csproj --locked-mode
dotnet build examples/engine/UareuEngine.csproj -c Release --no-restore
```

Keep its output directory intact. Pass its native apphost executable, rather
than its DLL, and an independently confirmed output pixel scale:

```sh
cargo run -p fingerprint-studio -- \
  --engine /absolute/path/to/UareuEngine \
  --ppi 500 \
  --threshold 40
```

The `500` above is an example, not a measurement of this decoder's output.
Matching requires both an engine and explicit scale. `--ppi` accepts 250–2000;
`--threshold` must be positive and finite. Threshold 40 is a demo policy, not a
validated security setting. Scores are SourceAFIS scores.

For experimental macOS trials only:

```sh
./scripts/run-fingerprint-studio-sourceafis.sh
```

This uses `--experimental-ppi 500`, displayed as **nominal, unverified**, and
threshold 40. It creates `dist/Fingerprint Studio SourceAFIS.app`; keep that
bundle inside the repository's `dist/` directory so its launcher finds the
helper. A nominal datasheet resolution does not establish effective capture scale.

## Capture, enroll, verify

1. Choose **Capture** for a standalone image.
2. Choose **Enroll**, then **Start enrollment**. Place, lift, and reposition the
   same finger three times; impressions are added automatically.
3. Lift after the third impression. Choose **Verify**, then **Start verification**.
   Each new placement is compared; lift between scans and choose **Stop verification**
   when finished. Matches and non-matches both continue scanning.
4. **Clear session** discards the preview and enrollment when idle.

Rejected impressions leave accepted samples intact. **Stop enrollment** pauses;
**Resume enrollment** continues the session. Switching modes requests a safe stop
without starting another operation. Resuming may require an extra lift/place cycle
to discard a placement held during the pause.

Images and templates stay in memory and are not exported. Helper requests use
private temporary files removed after each operation; this is not secure erasure.
Contrast is a pixel diagnostic. The app provides neither liveness detection nor
operating-system biometric login.

## Controls and appearance

Tab/Shift+Tab navigate controls, arrow keys select modes, and Enter/Space activate
focused controls. Escape stops; Cmd/Ctrl+W closes and Cmd/Ctrl+Q quits. **Details**
shows scale, contrast, helper configuration, and scores. The Window menu offers
standard/compact sizing and full screen; narrow windows stack the workspace.

Use the appearance icon to cycle **Light → Dark → System**, or choose from the
Appearance menu. System follows OS changes. The preference persists separately
from biometric data; on macOS it is stored in
`~/Library/Application Support/Fingerprint Studio/appearance`. Themes preserve
active work and original grayscale pixels.

## Build and checks

GPUI is pinned to the Zed revision in [Cargo.toml](Cargo.toml); `runtime_shaders`
avoids requiring the offline Xcode Metal compiler on macOS. Native GPUI platform
dependencies still apply; Linux/Windows need native-platform and hardware checks.

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-targets
```

Both macOS launchers accept `--build-only`. Regenerate the icon from `assets/`
with `./scripts/build-fingerprint-studio-icon.py`.
