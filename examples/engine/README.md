# Extended SDK usage and feature gaps

The C contract is [uareu.h](../../include/uareu.h); Rust modules expose the same
operations. [Advanced.cs](../csharp/Advanced.cs) is an executable C# example.
These extensions preserve existing capture signatures and C numeric statuses.
They use this project's ABI, not the proprietary U.are.U binary interface.

## Build and run

Requires .NET 10, x64 Windows/MSVC for the DLL, and the existing WinUSB setup.
The recognition helper is optional and runs locally as a bounded child process.

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

The helper uses SourceAFIS 3.14.0's raw-pixel constructor. ImageSharp appears only
in NuGet dependency metadata; all its compile, build and runtime assets are
excluded. No encoded-image decoder is used or distributed. Keep the helper's
complete output directory together, including the accompanying notices.

## Capture sessions, jobs and reports

`Reader::capture_next` borrows the reader and retains its interface. Existing
consuming methods remain available. In C, `uareu_reader_open_ex` with
`UAREU_OPEN_PERSISTENT` retains ownership between captures; the original open
function keeps its prior reopen behavior. Only one operation may lease a reader.

`uareu_capture_continuous` waits for successive placements and removals; it is
not a video stream. Each image callback owns its image handle and must free it.
Return zero to continue or one to stop. Callbacks execute synchronously and must
not throw across the native boundary. Cancellation returns `UAREU_CANCELLED`.

`uareu_capture_start` reserves the reader and starts an owned native worker.
`uareu_task_wait` reports pending or consumes the completion exactly once; a
successful completion transfers an image handle to the caller. A failed
completion copies structured error details to the waiting thread. Cancel may
run on another thread. Task free cancels and joins its worker. Reader close
requests cancellation; retain/join the task before unloading the DLL. Worker
threads never call foreign callbacks. Rust offers `CaptureTask` with equivalent
cancel/wait/close semantics. No asynchronous APIs upload biometric data.

Data handles contain immutable JSON. Probe/copy using `uareu_data_get_json`, then
release with `uareu_data_free`. `uareu_image_assess` caches one report, including
optional NFIQ 2 output; repeated reads do not rerun the scorer. NFIQ 2 requires
its executable/model and suitable explicitly known scale. Advisory contrast,
clipping and texture feedback are not fingerprint validity or liveness tests.
Enhancement creates a separate image and records processing; retain originals.

The explicit register-07 probe observes connectivity at that instant and a
low-power bit. It does not establish readiness, calibration or live connectivity
later. Cached lifecycle status is separate. Soft recovery performs the existing
stop/low-power sequence and rearms queues; it is not a vendor reset/calibration.
Capture failures preserve their primary cause and optional secondary cleanup
failure; unknown backend failures are not classified by parsing error text.

## Local recognition

Extraction requires known square resolution (250–2000 PPI); image dimensions and
runtime are bounded. Unknown capture resolution is rejected. An optional PPI
argument is a caller declaration recorded with provenance, not a measurement.
Do not assign 500 PPI to this reader merely to make extraction run.

```powershell
# Only use a source whose scale has independently been established:
dotnet $example --extract known-scale.png template.json $engine
dotnet $example --compare probe.json candidate.json 40 $engine
dotnet $example --enroll enrollment.json 40 $engine sample1.json sample2.json
dotnet $example --verify-enrollment probe.json enrollment.json 40 $engine
dotnet $example --identify probe.json 40 $engine candidate1.json candidate2.json
```

The threshold 40 above is syntax illustration, not a validated security setting.
Choose thresholds using representative data and measured false match/non-match
rates. Enrollment stores 2–10 mutually consistent templates; it does not fuse
minutiae. Verification takes the maximum sample score. Identification ranks a
bounded gallery. Scores are SourceAFIS scores, not vendor scores or FMR values.
Templates retain engine version/build hashes, source scale, region, processing
and optional image hash. Hashes describe provenance, not engine trust. Native
caches are version bound; preserve original images for future extraction.

Identification and enrollment verification batch comparisons when the serialized
request fits 16 MiB. The helper prepares the probe once and returns ordered
scores; Rust retains thresholding, stable ranking and maximum-score policy.
Requests above the aggregate limit keep the previous per-comparison path, with
the same overall deadline. There is no persistent matcher process.

The private `compare` request keeps `probe` and `candidate` and optionally adds
`additional_candidates`. Updated helpers return the existing `result.score`
plus ordered `result.additional_scores`. Old SDKs still use updated helpers.
An older helper ignores the additional request field; a valid response without
`additional_scores` retains its first score and processes the remainder through
the old path. Malformed batch scores, engine mismatch, process failure and
timeouts fail the operation rather than triggering a retry. The gallery remains
limited to 1000 candidates and each request/response remains bounded to 16 MiB.

See [recorded synthetic throughput checks](../../docs/validation/recognition-performance.md).

## Interchange

`--export-template TEMPLATE.json OUT.fmr FORMAT QUALITY IMPRESSION ENGINE` and
`--import-template IN.fmr OUT.json FORMAT ENGINE` support `iso19794_2_2005` and
`ansi378_2004`. This is a single-view, no-extension subset with strict lengths,
coordinates, angles and quality checks. Export requires explicit quality and
impression; unknown quality can be represented by zero. Import records declared
header scale. It rejects unsupported views/extensions and ambiguous minutiae.
Conversion does not imply certification or vendor compatibility.

`--image-record-encode INPUT.png OUT.fir OPTIONS.json` and
`--image-record-decode IN.fir OUT.png` support raw, 8-bit, single-view
ISO 19794-4:2005 records. Options require `capture_device_id`,
`acquisition_level`, `scale_units` (1=PPI, 2=pixels/cm), `scan_resolution_x`,
`scan_resolution_y`, `finger_position`, `quality`, and `impression`. Supply actual
acquisition facts; there are no inferred hardware defaults. Output image scale
must already be known. Decoder provenance describes the embedded declaration.

`--wsq-encode INPUT.png OUT.wsq CWSQ.exe DWSQ.exe [BITRATE]` and
`--wsq-decode IN.wsq OUT.png CWSQ.exe DWSQ.exe` adapt separately installed NBIS
executables. Encoding requires explicit 500 PPI and suitable geometry. Decoding
uses NISTCOM scale only when present and valid. Processes, files, dimensions and
timeouts are bounded. No NBIS binaries are bundled. This adapter has not had an
end-to-end codec test on this machine; availability is conditional.

## Verification and remaining gaps

See the [latest implementation gap review](../../docs/IMPLEMENTATION_GAPS.md) for
the detailed vendor API comparison and outstanding validation work.

Run `scripts/test-windows-dll.ps1` for C/C# ABI checks, raw-image/report/FIR
round trips, synthetic SourceAFIS extraction/comparison/enrollment/identification,
and single-view ANSI/ISO minutiae conversions. Add `-HardwareCancellation` for
exclusive retained sessions, recovery, async jobs, phase status and cancellation
using a connected reader without placing a finger. Synthetic patterns do not
validate biometric accuracy, reader output resolution, or standards conformance.

| Area | Implemented | Still missing or unverified |
|---|---|---|
| Discovery/capture | Rust, C DLL, C#, persistent sessions, continuous placements, async jobs | Other reader families, true video streaming |
| Status/errors | SDK capability profile, cached lifecycle, explicit register probe, typed errors and cleanup details | Authoritative device capability/readiness queries |
| Resolution | Unknown by default; declaration/measurement provenance | Actual output DPI of the connected reader |
| Quality | Diagnostics, advisory feedback, optional NFIQ 2, separate enhancement | Vendor quality flags/correction, PIV compliance, liveness |
| Recovery/control | Stop/low-power sequence and queue rearming | Vendor reset/calibration, LED/parameter controls |
| Recognition | Optional SourceAFIS extraction, compare, rank, multi-template enrollment | Vendor algorithm parity, fusion, measured FMR/accuracy |
| Formats | Bounded ISO/ANSI minutiae and raw ISO image subsets | ANSI 381 images, multiview/extensions, compressed ISO records, certification |
| WSQ | Optional bounded NBIS process adapter | Actual backend round trip and interoperability validation |
| Distribution | C header/examples, locked helper dependencies, attribution | Vendor ABI compatibility, vendor Windows runtime integration |

Unsupported/unverified hardware capabilities remain unknown. Closing those gaps
requires documented protocol evidence, a compatible vendor runtime, calibrated
test targets, additional hardware or a representative biometric evaluation set.
No placeholder successful operations stand in for those capabilities.

References: [SourceAFIS .NET](https://sourceafis.machinezoo.com/net),
[native template compatibility](https://sourceafis.machinezoo.com/template),
[ISO minutiae layout](https://templates.machinezoo.com/iso-19794-2-2005),
[ANSI minutiae layout](https://templates.machinezoo.com/ansi378-2004), and
[NIST NBIS](https://www.nist.gov/services-resources/software/nist-biometric-image-software-nbis).
Format descriptions by Robert Važan are licensed CC BY 4.0; implementation is
independent. ISO image layout was also checked against JMRTD's `FingerInfo` and
`FingerImageInfo` sources. No vendor headers/binaries are redistributed.
