# Implementation gaps against the supplied U.are.U SDK

Reviewed 2026-10-25 against the supplied Linux SDK headers `dpfpdd.h`, `dpfj.h`,
`dpfj_quality.h`, and `dpfj_compression.h`, and the current Rust/C/C# implementation.
This compares API families and behavior, not binary compatibility or certification.
Vendor APIs may themselves depend on reader capabilities; their presence in a
header does not establish support in the connected 05ba:000a reader.

This SDK implements discovery, capture/cancel, retained sessions, repeated finger
placements, owned async jobs, cached status, an explicit register probe, soft
recovery, diagnostics, optional NFIQ 2, optional SourceAFIS recognition, and limited
interchange. Those foundations are implemented; the gaps below describe their
remaining limits. See [usage and ownership](../examples/engine/README.md) and
[the C ABI](../include/uareu.h).

## Remaining functionality and parity gaps

| Area / vendor reference | Current implementation | Remaining gap / evidence needed |
|---|---|---|
| Output resolution | Unknown captures; explicit provenance for declarations/imported headers | Actual decoder output DPI remains unverified. Needs a calibrated target or an authoritative runtime/profile verified against this output. USB ID and nominal sensor DPI are insufficient. |
| Device capabilities: `dpfpdd_get_device_capabilities` | SDK implementation profile separate from unknown device fields | No authoritative hardware capability, supported resolution or processing-mode query. Needs documented or observed device protocol. |
| Device status: `dpfpdd_get_device_status` | Cached lifecycle plus explicit register-07 connectivity/low-power observation | No vendor readiness, calibration-needed, finger-quality or live unplug notifications. An observation can become stale. |
| Device identity | USB descriptors and revision when available | Vendor hardware/firmware version and full vendor device-information structures are not exposed. |
| Open modes: `dpfpdd_open_ext` | Exclusive interface ownership, optionally retained | Vendor access-priority/cooperative-open semantics are not implemented. Persistent ownership is a separate concept. |
| Async capture: `dpfpdd_capture_async` | Owned task, cancel, wait/poll, join on free | No vendor callback ABI parity. Applications use tasks or the synchronous capture phase callbacks. |
| Streaming: start/get/stop stream | Continuous placements/removals | No true frame/video streaming API or verified streaming protocol. |
| Reset/calibration: `dpfpdd_reset`, `dpfpdd_calibrate` | Stop/low-power sequence, queue rearming and a follow-up probe | No vendor reset, calibration procedure, calibration storage or bus-reset equivalent. |
| LED and parameters: led_config/led_ctrl/get/set_parameter | Not exposed | Commands, supported parameter IDs and reader support need verification. |
| Processing: DPFPDD_IMG_PROC modes | Decode/invert/rotate and separate percentile display enhancement | No vendor default/PIV/enhanced/enhanced-2 processing, dewarping or calibrated correction. No PIV certification. |
| Capture quality flags | Intensity/texture diagnostics with advisory warnings | No vendor quality-bit equivalence, validated dry/wet/fake-finger classifier or liveness result. Keep hardware liveness unknown. |
| Quality: `dpfj_quality_nfiq_from_raw/fid` | Optional external NFIQ 2; known suitable scale required | No bundled scorer, legacy NFIQ 1/Aware engine parity or FID-view quality selector. NFIQ 2 is not interchangeable with the vendor quality algorithms. |
| Extraction/matching: dpfj create_fmd/compare/identify | Optional local SourceAFIS, explicit scale and threshold, bounded gallery | No vendor algorithm, proprietary DP template encoding, vendor score/FMR equivalence, or measured recognition accuracy. |
| Recognition throughput | A local helper process per comparison, with an overall operation deadline | No persistent matcher/index or batched gallery protocol. Startup and repeated probe preparation limit useful gallery size under the deadline. |
| Enrollment: start/add/create/finish_enrollment | Multi-template container with pairwise consistency and maximum-score verification | No fused enrollment FMD or vendor incremental enrollment state machine. |
| Minutiae conversion/record editing | Single-view ANSI 378:2004 and ISO 19794-2:2005, no extensions | No multiview/extensions, other editions, proprietary conversions or vendor record/view getter/setter API parity. No external conformance validation yet. |
| Fingerprint image records / raw conversion | Single-view raw 8-bit ISO 19794-4:2005 subset | No ANSI 381, proprietary DP FID, compressed/multiview records or general vendor raw/FID conversion parity. |
| Compression: dpfj compression lifecycle | Optional external NBIS WSQ adapter with bitrate | No bundled/in-process codec, target-size control or complete FID compression API. Actual cwsq/dwsq round trip remains unverified here. |
| Readers/platforms | USB profile 05ba:000a; Windows reader exercised locally | Other USB families are not implemented. Linux/macOS hardware behavior and additional reader revisions still need physical verification. |
| Windows distribution | DLL/header/C# examples and build checks | WinUSB association still needs per-machine setup. No signed installer or complete redistributable release workflow for the optional .NET helper/NBIS/NFIQ toolchain. |
| Vendor compatibility | Own additive C ABI v1, existing capture signatures retained | Not a replacement implementing the dpfpdd/dpfj binary interfaces. SourceAFIS caches are version bound; added Rust enum variants can affect exhaustive downstream matches. |

## Validation still required

The review passed Rust workspace tests/doctests, strict Clippy, formatting,
synthetic fixture and dependency-notice checks, release builds, native C/C#
integration, malformed-template helper tests, synthetic recognition/interchange
round trips, and connected-reader probe/recovery/session/async cancellation checks
on Windows x64. The external NFIQ reference test remains ignored. These checks
do not close the following validation gaps.

* Establish output scale before any real captured-image NFIQ 2 or SourceAFIS result
  can be considered tested. Synthetic virtual images have explicitly declared
  scale; they do not establish this reader's DPI or biometric performance.
* Run real NBIS encode/decode with representative images, NISTCOM variants,
  truncation/errors, and interoperability checks. Current tests cover adapter
  bounds and scale/envelope rejection, not a working codec backend.
* Check ANSI/ISO records against independently generated valid/invalid vectors
  and independent consumers. Internal round trips do not prove conformance.
* Measure false match/non-match rates on representative, appropriately obtained
  data before choosing operational thresholds. Test enrollment rejection and
  identification behavior at realistic gallery sizes. Thresholds in examples
  are illustrative or synthetic-test values.
* Exercise physical unplug/reconnect during capture, probe, recovery and async
  jobs; concurrent close/cancel/wait; repeated scans with actual images; and
  prolonged sessions. Cancellation tests without a finger do not cover those
  hardware failure paths. SDK operation deadlines do not bound OS open/close or
  arbitrary caller callbacks.
* Run the ignored external NFIQ reference/conformance check with its required
  fixtures/backend. Passing ordinary tests is not NFIQ conformance evidence.
* Perform reader tests on Linux/macOS, other revisions and additional devices.
  CI can check builds and deterministic behavior without establishing hardware
  support. Current validation here is Windows x64 only.

## Review corrections included in this revision

* Disconnected continuous/probe/recovery sessions are discarded instead of being
  retained for future operations; the default and persistent capture paths share
  that retention rule, including disconnections reported only by cleanup.
* Multiple cleanup/recovery failures are retained in order while preserving the
  original acquisition error and its numeric C status.
* Recovery includes a fresh status probe in both Rust and C. A successful probe
  cannot clear a failure from the preceding recovery writes or queue rearm.
* Failed probe opens and panicking reader operations finish the correct cached
  status instead of leaving a probing state or reporting an unrelated capture.
* The helper bounds stdin before parsing, requires every native-cache field and
  rejects out-of-image coordinates/directions before calling SourceAFIS.
* ISO image imports validate resulting metadata, including the SDK's supported
  resolution range. C ownership documentation includes persistent sessions.

## Suggested next work

1. Add independent format vectors and real NBIS integration tests; these are
   actionable without claiming new reader capabilities.
2. Add hardware unplug/reconnect and repeated real-image session tests, including
   a record of reader revision, OS, USB driver and test outcomes.
3. Establish actual output scale when suitable runtime evidence or a calibrated
   target becomes available; then validate real-image NFIQ 2 and recognition.
4. Implement additional hardware commands or reader families only from verified
   protocol evidence. Preserve unknown capability fields until that evidence exists.

The lack of a Windows vendor runtime and calibrated target was already established
in this project session. Those dependencies remain unresolved; no successful
placeholder stands in for the missing hardware behavior.
