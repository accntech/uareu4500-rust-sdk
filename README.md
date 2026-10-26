# UareU Rust SDK

Capture, preview, enroll, and verify fingerprints with a U.are.U 4000B/4500 USB
reader. Use **Fingerprint Studio** for a visual workflow, the **command-line tool**
for capture and image inspection, or the **SDK** to bring fingerprint features
into your own application.

An experimental project targeting Windows, Linux, and macOS.

## What you can do

### Capture and preview fingerprints

Connect a reader and capture a grayscale fingerprint image. Fingerprint Studio
shows the scan in a native desktop workspace; the command-line tool saves it as
a PNG for use in other workflows.

Need multiple scans? Continuous capture waits for you to lift your finger before
taking the next image. Saved captures use unique filenames without overwriting
existing files.

### Enroll a finger in three impressions

Fingerprint Studio guides you through placing, lifting, and repositioning the
same finger three times. Accepted impressions remain in the session if a later
scan is rejected. Pause enrollment and resume when ready, or clear the session
to start again.

### Verify each new finger placement

Once enrolled, switch to continuous verification. Each new scan is compared with
the enrolled finger and displays a match or non-match result. Scanning continues
until you stop it.

Enrollment and verification use an optional local **SourceAFIS** helper and
require an explicit image-resolution declaration. See
[matching setup](apps/fingerprint-studio/README.md#configure-matching).

### Inspect image quality

Check saved images for contrast, intensity, clipping, and available resolution
information without connecting a reader. Optional **NFIQ 2** integration adds
fingerprint quality scoring for images independently established as 500 PPI.

### Keep fingerprint processing local

Capture does not upload biometric data. Fingerprint Studio keeps scan previews
and enrollment templates in memory for the current session, and **Clear session**
discards them when idle. Matching runs through a local helper, whose temporary
request files are removed after each operation.

### Build fingerprint features into your application

Use the Rust library or C interface, with C# examples, to add reader discovery,
capture, quality inspection, and optional recognition to your own software.
Additional SDK features include asynchronous capture, reader status monitoring,
and selected fingerprint image and template interchange formats.

## Fingerprint Studio

A focused desktop workspace with three modes:

| Mode | Workflow |
| --- | --- |
| **Capture** | Take a fingerprint scan and view the grayscale image. |
| **Enroll** | Collect three impressions of one finger, with pause and resume. |
| **Verify** | Compare repeated finger placements with the current enrollment. |

The app includes light, dark, and system appearance, keyboard navigation,
compact and full-screen layouts, and a details panel for scan information and
matching scores.

[Explore Fingerprint Studio →](apps/fingerprint-studio/README.md)

## Get started

Connect a supported reader and complete the
[USB setup for your operating system](SDK_GUIDE.md#build-and-capture).
From the repository root, launch the desktop app:

```sh
cargo run -p fingerprint-studio
```

Or capture an image from the command line:

```sh
cargo build --locked --release
cargo run --locked --release -p uareu -- list
cargo run --locked --release -p uareu -- capture --output fingerprint.png
```

Basic capture works without a recognition helper. To enroll and verify, follow
[the matching setup guide](apps/fingerprint-studio/README.md#configure-matching).

## Current scope

Supported readers are U.are.U 4000B/4500 devices with USB ID **05ba:000a**.
Physical-reader behavior still needs verification on each operating system;
an observed macOS capture stall remains unresolved.
Capture resolution is unknown by default; declaring a resolution does not measure
or resample the image.

The project does not provide liveness detection or operating-system biometric
login. Matching thresholds are demonstration settings, with accuracy and security
validation still outstanding. Temporary-file cleanup is not secure erasure.

## Documentation

- [SDK guide](SDK_GUIDE.md) — installation, CLI options, Rust API, C/C# integration, and packaging.
- [Fingerprint Studio](apps/fingerprint-studio/README.md) — desktop workflows, matching setup, and controls.
- [Advanced SDK examples](examples/engine/README.md) — recognition, asynchronous capture, and interchange.
- [Validation status](docs/validation/README.md) — evidence and checks completed so far.
- [Implementation gaps](docs/IMPLEMENTATION_GAPS.md) — remaining limitations and validation work.

Licensed **GPL-3.0-or-later**. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
