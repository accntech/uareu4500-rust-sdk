# Advanced SDK and local recognition

Use [uareu.h](../../include/uareu.h) for the C ABI contract and
[Advanced.cs](../csharp/Advanced.cs) for executable C# examples. This is the
project's own ABI, not the proprietary U.are.U binary interface.

## Build and run

The Windows example needs .NET 10, x64 Windows/MSVC, and
[WinUSB setup](../../SDK_GUIDE.md#windows-usb-setup). Recognition uses an optional
local SourceAFIS 3.14.0 helper; capture needs no helper.

```powershell
cargo build --locked --release
dotnet build examples/csharp/UareuExample.csproj -c Release
dotnet restore examples/engine/UareuEngine.csproj --locked-mode
dotnet build examples/engine/UareuEngine.csproj -c Release --no-restore
$env:UAREU_DLL_PATH = (Resolve-Path target/release/uareu.dll).Path
$example = 'examples/csharp/bin/Release/net10.0/UareuExample.dll'
$engine = (Resolve-Path examples/engine/bin/Release/net10.0/UareuEngine.exe).Path
dotnet $example --capture-async fingerprint.png
dotnet $example --quality fingerprint.png
dotnet $example --enhance fingerprint.png enhanced.png
dotnet $example --probe
```

Keep the helper's complete output directory and notices together. It uses
SourceAFIS's raw-pixel constructor; ImageSharp compile/build/runtime assets are
excluded and no encoded-image decoder is distributed.

## Sessions and ownership

`Reader::capture_next` borrows and retains a reader. C callers can use
`uareu_reader_open_ex` with `UAREU_OPEN_PERSISTENT` for the same ownership policy.
Only one operation may lease a reader.

`uareu_capture_continuous` waits for finger removal between captures. Each image
callback owns its handle and must free it; return zero to continue or one to stop.
Phase/image callbacks run synchronously. Completion callbacks run on the worker.
Foreign callbacks must not throw across the native boundary.

`uareu_capture_start` reserves the reader and starts a worker. `uareu_task_wait`
reports pending or consumes completion once, transferring an image handle on
success and copying structured errors to the waiting thread on failure. Cancel
may run on another thread. Task free cancels and joins; reader close requests
cancellation. Join/free outstanding tasks before unloading the DLL.

JSON data handles are immutable: probe/copy with `uareu_data_get_json`, then free
with `uareu_data_free`. Image enhancement creates a new image; retain originals.
Cached status and explicit register probes do not establish ongoing physical
readiness, calibration, or resolution. Soft recovery is not vendor calibration.

## Recognition

Extraction requires known square resolution, 250–2000 PPI. A PPI argument is a
caller declaration, not a measurement. Do not assign nominal reader PPI merely
to enable matching.

```powershell
# Use an image whose output scale is independently established.
dotnet $example --extract known-scale.png template.json $engine
dotnet $example --compare probe.json candidate.json 40 $engine
dotnet $example --enroll enrollment.json 40 $engine sample1.json sample2.json
dotnet $example --verify-enrollment probe.json enrollment.json 40 $engine
dotnet $example --identify probe.json 40 $engine candidate1.json candidate2.json
```

Threshold 40 illustrates syntax. Select a production threshold using representative
data and measured false match/non-match rates. Enrollment retains 2–10 mutually
consistent templates without fusion; verification uses the maximum sample score.
Identification ranks up to 1000 candidates. Scores are not vendor scores or FMR values.

Templates retain engine/build identity, source scale, region, and processing
provenance. Preserve original images for future extraction. Comparisons batch
when requests fit 16 MiB; larger galleries and older helpers use bounded
per-comparison fallback under the same overall deadline. Malformed results,
engine mismatch, helper failure, and timeout fail the operation. No persistent
matcher process or biometric upload is used.

## Interchange and optional engines

The C# example exposes these single-view conversions:

- `--export-template TEMPLATE.json OUT.fmr FORMAT QUALITY IMPRESSION ENGINE`
- `--import-template IN.fmr OUT.json FORMAT ENGINE`
- `--image-record-encode INPUT.png OUT.fir OPTIONS.json`
- `--image-record-decode IN.fir OUT.png`
- `--wsq-encode INPUT.png OUT.wsq CWSQ.exe DWSQ.exe [BITRATE]`
- `--wsq-decode IN.wsq OUT.png CWSQ.exe DWSQ.exe`

Template formats are `iso19794_2_2005` and `ansi378_2004`. The single-view example
rejects unsupported extensions; the Rust/C SDK also exposes multiview and opaque
extension APIs. Quality/impression fields and image-record acquisition metadata
must be supplied explicitly. Record options require `capture_device_id`,
`acquisition_level`, `scale_units` (1=PPI, 2=pixels/cm), `scan_resolution_x`,
`scan_resolution_y`, `finger_position`, `quality`, and `impression`.

The SDK supports bounded ISO 19794-4:2005 image subsets and ANSI381 raw8 records.
External NBIS tools provide WSQ encode/decode; encoding requires explicit 500 PPI
and suitable geometry. External OpenJPEG supports lossless grayscale raw J2K;
raw codestreams do not provide physical DPI. Optional NFIQ2 and legacy NFIQ1 are
separate score scales. No external engine binaries are bundled. Format conversion
does not establish certification, matching accuracy, or vendor compatibility.

## Verify and assess limits

Run `./scripts/test-windows-dll.ps1` for device-free C/C# checks. Add
`-HardwareCancellation` for connected-reader lifecycle checks without placing a
finger. Real-engine reference tests in `tests/` are opt-in and specify their
required executables/data. Synthetic patterns do not validate biometric accuracy
or actual reader resolution.

See [implementation gaps](../../docs/IMPLEMENTATION_GAPS.md) and the
[validation index](../../docs/validation/README.md) for evidence and remaining work.
Device readiness/capabilities, vendor quality/correction, calibration, liveness,
PIV, true video streaming, other reader families, and vendor algorithm/ABI parity
remain unsupported or unverified.

References: [SourceAFIS .NET](https://sourceafis.machinezoo.com/net),
[template compatibility](https://sourceafis.machinezoo.com/template),
[ISO minutiae layout](https://templates.machinezoo.com/iso-19794-2-2005),
[ANSI minutiae layout](https://templates.machinezoo.com/ansi378-2004), and
[NIST NBIS](https://www.nist.gov/services-resources/software/nist-biometric-image-software-nbis).
Format descriptions by Robert Važan are CC BY 4.0; the implementation is independent.
No vendor headers/binaries are redistributed.
