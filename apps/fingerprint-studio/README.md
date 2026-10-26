# Fingerprint Studio (GPUI)

A small native desktop app using this repository's real USB fingerprint SDK.
Includes a custom draggable titlebar with native macOS traffic lights, a real
Dock/Finder icon, visible Capture / Enroll / Verify modes, reader refresh,
cancellable capture, grayscale preview, continuous three-impression enrollment,
verification, and clear-session controls. All reader and recognition operations
run on a worker thread; closing cancels capture and waits for worker cleanup.
Recognition already in progress may take up to its bounded extraction and
comparison deadlines to finish before exit.

## Run

```sh
cargo run -p fingerprint-studio
```

On macOS, a local app bundle with a proper app name can also be built and run:

```sh
./scripts/run-fingerprint-studio-macos.sh
```

USB capture and preview need no recognition helper. Connect a supported
U.are.U 4000B / 4500 (`05ba:000a`); refresh discovers readers and uses the first
one listed by the SDK. If several are connected, disconnect the others to use a
particular reader. Existing OS USB permissions/driver requirements still apply;
see the root README and `udev/70-uareu.rules`.

## Enrollment and verification

For experimental testing on this Mac, build/check the helper and open the app
with matching configured using:

```sh
./scripts/run-fingerprint-studio-sourceafis.sh
```

It also creates `dist/Fingerprint Studio SourceAFIS.app`, which you can reopen
in Finder with matching configured. Keep that bundle inside this repository’s
`dist` folder so its launcher can locate the helper.

This launcher passes `--experimental-ppi 500` and the demo threshold of 40.
The app labels the scale **nominal, unverified**. HID's
[current 4500 datasheet](https://www.hidglobal.com/sites/default/files/documentlibrary/bio-digitalpersona-4500-reader-ds-en_0.pdf)
lists 500 dpi; older Crossmatch sheets list 512 dpi. Neither establishes the
actual effective scale of this decoder's descriptor-expanded image. This
explicit experimental assumption enables enrollment trials without claiming
calibration or accuracy. The SDK records it as `caller_declared`; it is never
recorded as device-reported or independently measured. Default capture mode
still leaves scale unknown. For calibrated use, follow the confirmed-PPI setup
below instead.

Build the optional local .NET 10 SourceAFIS helper from `examples/engine`:

```sh
dotnet build examples/engine/UareuEngine.csproj -c Release
```

Keep the helper's output directory intact. Pass its native apphost executable,
not its DLL, and a pixel scale independently confirmed for your capture setup:

```sh
cargo run -p fingerprint-studio -- \
  --engine /absolute/path/to/UareuEngine \
  --ppi 500 \
  --threshold 40
```

Here `500` is only an example caller declaration. USB capture does **not** report
PPI, and the app never silently assumes it. Matching stays disabled until `--engine` and an explicit scale declaration
(`--ppi` or the experimental `--experimental-ppi`) are supplied. The executable is validated when used.
`--ppi` accepts 250–2000; `--threshold` must be positive and finite.

1. Select **Enroll** and choose **Start enrollment** once.
2. Place, lift, and reposition the same finger three times. The app adds each
   impression automatically and waits for removal between acquisitions.
3. Lift after the third impression, then select **Verify** and choose
   **Start verification**. The app matches each new placement continuously;
   lift between scans and choose **Stop verification** when finished. Both matches
   and non-matches keep scanning, and the latest result remains visible.
4. Select **Capture** for a standalone image. **Clear session** discards the
   preview and enrollment when no operation is running.

Inconsistent enrollment impressions are retried while accepted samples remain.
The continuous reader protocol observes finger removal between enrollment
acquisitions. **Stop enrollment** pauses without losing accepted samples;
**Resume enrollment** continues the same session. Switching modes requests a
safe stop and never starts a second operation automatically. A held placement
at a pause boundary is discarded before another independent impression is
accepted. This conservative behavior can require an extra lift-and-place cycle
after resuming. Helper work already underway finishes within its deadline.

Images/templates stay in the session and are not exported. The recognition SDK
uses private temporary request files, removed when its helper operation ends;
this is not a guarantee of secure erasure. Scores are SourceAFIS scores. The
default threshold of 40 is a demonstration policy, not a calibrated security
claim. Image contrast is a pixel diagnostic, not NFIQ, fingerprint validity, or
liveness. No liveness detection or operating-system biometric login is provided.

The visible mode selector replaces capture/enrollment command shortcuts. Tab
and Shift+Tab navigate enabled controls, arrow keys move between modes, and
Enter/Space activate the focused control. Escape stops an operation; standard
Cmd/Ctrl+W and Cmd/Ctrl+Q close/quit. The window opens at 840×570 logical
pixels and can resize down to 360×480.
The Window menu offers Standard Size (840×570), Compact Size (420×720), and
full-screen controls. At 700px and above, a bounded landscape preview sits beside
the instructions, current phase/result, and actions. Narrower windows stack into
one column with live feedback before the preview and Start/Stop pinned below the
scrolling workspace. Related controls remain grouped and top-aligned. Image
previews preserve the captured aspect ratio. **Details** reveals pixel scale,
contrast, helper configuration, and match scores; experimental scale is still
flagged in the reader toolbar.

## Minimalist layout

The workspace follows the compact grouping and restrained navigation references
from [Things](https://culturedcode.com/things/blog/2025/09/things-for-os-26/) and
[Linear](https://linear.app/now/behind-the-latest-design-refresh). It has no
vertically centered work region or expanding instruction spacer. Empty-state
captions and the detached session-status footer were removed; task feedback now
appears beside the controls. Continuous enrollment and verification use the
existing worker unchanged.

## Appearance

Use the small appearance icon beside Refresh to cycle **Light → Dark →
System**, or choose a mode directly from the **Appearance** menu. The sun, moon,
and monitor icons show the selected preference; hover to see the current and
next mode. Refresh and appearance use compact 32px buttons with 16px vectors.
System is the default and follows OS light/dark changes while the app is open. Explicit Light or Dark stays fixed.
The preference is remembered independently of biometric session data. On macOS,
it lives in `~/Library/Application Support/Fingerprint Studio/appearance`.

All controls, text, borders, status colors, and the transparent fingerprint mark
use the selected palette. Captured fingerprints retain their original grayscale
pixels. The native app icon has curved asymmetric ridges, a pale tile, and small
16/32px variants for legibility. Theme changes preserve active scanning,
enrollment samples, and the last completed verification result.

## UI review

The incumbent interface scored **19/40** in an independent Nielsen heuristic
review. Its key problems were a six-action manual enrollment sequence,
recall-heavy command shortcuts, oversized portrait framing for landscape
captures, and non-native glyph window controls. The revised workflow addresses
those with explicit modes, one continuous enrollment operation, compact
window sizing, native macOS chrome, and a shared fingerprint icon. This score
records the original critique; it is not a post-change usability certification.
The native GPUI source cannot be inspected by a web-markup design detector.

Original-interface scores (subjective expert assessment; 0–4 per heuristic):

| Heuristic | Score | Original issue |
| --- | ---: | --- |
| System status | 3 | Capture phases were visible |
| Match to real workflow | 2 | Manual capture/add alternation |
| Control and freedom | 2 | No visible mode or intention |
| Consistency and standards | 1 | Non-native glyph window controls |
| Error prevention | 2 | Finger removal was only instructed |
| Recognition over recall | 1 | Shortcut footer emphasized commands |
| Efficiency | 1 | At least six actions for enrollment |
| Minimalist design | 2 | Oversized headline and portrait frame |
| Error recovery | 3 | Accepted samples survived rejection |
| Help and documentation | 2 | Prerequisites lacked nearby explanations |
| **Total** | **19/40** | **Workflow and native affordances needed work** |

The first-time user faced unexplained disabled actions; the repeat operator
had to alternate between the scanner and two buttons. Four of eight cognitive
load checks failed in the critique: task selection, competing actions, command
recall, and disconnected steps. Clear previews, session-only storage messaging,
and phase feedback were retained. The mechanical native assessment confirmed
missing bundle icons, blank window naming, and oversized minimum dimensions;
a web-markup detector is inapplicable to GPUI. No post-change usability score
or screen-reader certification is claimed.

The icon source is in `assets/`. Regenerate it with
`./scripts/build-fingerprint-studio-icon.py`. Both macOS launchers support
`--build-only` to package and sign without opening the app.

## Build notes

GPUI is pinned to official Zed commit
`cc9af8d036270ad39953e6bc26e5167f84989af3`. The crates.io GPUI 0.2.2 package pins
`core-foundation =0.10.0`, conflicting with nusb 0.2.9's `^0.10.1`; this official
GPUI revision allows a compatible version without changing the SDK. The
`runtime_shaders` feature avoids requiring the offline Xcode Metal compiler on
macOS. GPUI platform build requirements still apply. macOS is validated locally;
Linux/Windows require their native platform dependencies and hardware validation.

```sh
cargo fmt --all --check
cargo clippy --all --workspace
cargo test --all --workspace
```
