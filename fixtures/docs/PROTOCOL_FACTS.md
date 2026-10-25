# UareU 4500 Rust SDK interoperability specification

This specification records community-derived wire formats and mathematical
behavior, plus project-specific validation and lifecycle policies. It is not a
manufacturer specification or a record of independently measured hardware tests.
The implementation remains experimental until tested on actual readers.

The facts were learned during prior examination of libfprint's uru4000 driver.
See the root NOTICE for the pinned reference, upstream copyright, license origin,
GPL conversion scope, and modification date. No clean-room or legal clearance
claim is made. Project documentation is GPL-3.0-or-later; third-party grants and
notices remain separate.

## USB identity and transfers

- Target USB vendor/product: 05ba:000a; 4000B and 4500 can share this identity.
- A capture interface identifies itself with class/subclass/protocol ff/ff/ff.
- Interrupt IN endpoint 81 and bulk IN endpoint 82 have 64-byte packets.
- Device-recipient vendor control request 04 accesses registers: register address
  is wValue, wIndex is zero. Request type c0 reads, 40 writes.
- Interrupt payloads begin with a big-endian 16-bit event: 56aa means scan power
  ready, 0101 finger present, 0200 finger absent, 0800 warns of a failed scan.
- Queue an interrupt read before changing power state. Continue draining events.
- Queue image transfer before requesting capture. Successful short transfers are
  complete frames, never fragments to concatenate with later completions.
- No AES challenge/response is specified for this device identity.

## Register behavior and session requirements

| Register | Meaning |
| --- | --- |
| 07 | Hardware status/power control |
| 33 | Image key-index and host-seed request, five bytes |
| 34 | Four-byte little-endian image-key response |
| 4e | Mode: 10 wait for finger, 12 wait for removal, 20 capture, 70 stop |
| f0 | 16-byte diagnostic data |

Status bit 7 denotes low power. A state with bits 7 and 2 set needs recovery:
write the status's low nibble, then observe status bit 0 becoming set. Repeated
observations need a short delay and a finite bound.

A power transition needs a low-power interval before activation. If status bit 7
is not set, write the observed status with bit 7 set. Activation writes the
observed low nibble, then checks for bit 7 clearing; repeat with delays if needed.
Power-ready event 56aa can arrive before the status check completes and must be
remembered. The earlier observations allow roughly 300 ms for this event and up
to three activation attempts. A bounded per-phase retry policy is required.

After power-ready, reading f0 returns 16 bytes. Mode 10 requests finger detection.
After event 0101, prepare the image read and request mode 20. Invalid image data
can be retried within a finite budget. A key-query I/O failure, disconnect, or
cancel is not merely an invalid frame and must terminate the operation.

All phases must honor an overall deadline and an AtomicBool cancellation flag.
Pending reads must be cancelled even when preflight fails. End a started session
by requesting stop (4e=70), then low power (07=80), and cancelling pending reads.
Attempt both stop/power writes even if one fails; always cancel. Cleanup needs its
own bounded allowance. Before any session activity, cancelled/expired requests
must issue no register writes. Public capture consumes a single session.

## Frame envelope and block coordinates

- 64-byte header, followed by row-major 8-bit sensor samples.
- Width 384; display canvas height 290. Capacity is 111424 total bytes.
- Header offsets 4..6 hold little-endian count of source rows, offset 6 key index.
- Fifteen two-byte descriptors occupy offsets 16..46. Each is (flags, row count).
- A zero row count terminates descriptors. Descriptor data after the declared
  source rows have been consumed is not part of the image.
- Source row count must be nonzero and at most 290; the actual transfer must
  include all declared source pixels. The 290-row case is a capacity-based
  acceptance policy, not yet an observation from physical hardware.
- Flag 01 means a descriptor contributes output rows without consuming source
  rows: those source rows will be reused by the next descriptor. Source reads
  and display expansion must both fit their respective capacities.
- Descriptors must account for exactly the declared source row count. Any
  unused output canvas is sensor value zero before display normalization.
- Normalization inverts sample intensity; optional 180-degree rotation changes
  display orientation. Neither transformation establishes biometric identity.

## Legacy sample scrambling

The first two source rows can be used to estimate scrambling: for each column
sum its two samples, calculate the integer mean of those 384 sums, then the
integer mean squared deviation. A mean squared deviation >= 5000 indicates scrambling; this is a
heuristic, not cryptographic validation. With fewer than two rows the heuristic
has no evidence of scrambling. Descriptor flag 02 alone is not a reliable
substitute for this heuristic on all variants.

Key exchange: write [key_index, four little-endian bytes of a fresh 31-bit host
seed] to 33, read four bytes from 34. XOR the little-endian response with the host
seed to obtain a 32-bit stream state. A new frame starts with its header's index.

Mathematical state transition: shift state right one bit; new bit 31 is the parity
of the old bits selected by mask 9248144d. For a scrambled sample, the mask byte's
bits 0..7 come respectively from state bits 4,8,11,14,18,21,24,29. Decode sample i
as transmitted sample i+1 XOR that mask, then advance the state once. The final
sample of each scrambled descriptor is implicitly zero; its state step still
occurs. This is legacy scrambling, not secure transport encryption.

The descriptor flags prescribe how stream state progresses:

| flags & 06 | Action over this descriptor's sample count |
| --- | --- |
| 00 | Advance state once per sample, without transforming pixels |
| 02 | Transform the descriptor's samples and advance state |
| 04 or 06 | Leave samples and state unchanged |

Flag 80 requests a fresh key before processing its descriptor: increment index
with u8 wraparound and perform another exchange. Initial exchange still uses the
header index. Flag 01 does not suppress stream-state advancement, even though it
does not consume source rows. Bounds must be validated before invoking key I/O.

### Overlapping transformations

All descriptor sample transformations affect a shared source buffer, in
transmission order. Display expansion observes the completed source buffer after
those transformations. In particular, when a non-consuming descriptor overlaps
with a later transformed descriptor, both display spans observe the final source
samples. Emitting a permanent display snapshot before a later overlapping write
would implement different behavior. This distinction is a functional format
requirement inferred from the prior technical investigation, not yet a separately
measured hardware observation.

## Phase correlation policy (project requirement)

Interrupts have no transaction identifiers. Before starting a new activation,
discard already queued events with a bounded drain and reset readiness. Only a
readiness event associated with the new activation may satisfy it; retain events
arriving during its status checks. Likewise, discard accumulated finger events
and reset finger state before issuing mode 10. Require presence evidence from
the new detection phase rather than accepting a latched startup event while an
absence event is still queued. Drain loops must honor cancellation/deadlines and
have a finite count bound. Software queue boundaries cannot establish perfect
hardware event correlation; physical-device timing tests remain necessary.

## Continuous capture sessions

Mode `4e=12` requests finger-removal detection; a subsequent `0200` event
confirms removal. This command/event mapping is recorded by the pinned upstream
uru4000 reference in NOTICE. Continuous capture keeps the initialized interface
open: after delivering an image it drains queued events, requests removal mode,
and requires new absence evidence before arming `4e=10` for another placement.
A repeated `0101` while a finger remains on the sensor does not permit a second
image. As with other phase boundaries, software event correlation remains
limited by device timing.

Continuous placement/removal waits have no inactivity deadline but poll with
bounded allowances and honor cancellation. Initialization and each image
acquisition receive separate `CaptureOptions::timeout` deadlines. The single-shot
API retains its overall-session deadline. A delivery callback can request a clean
stop; cancellation is also a clean stop in continuous mode. Callback, transport,
frame, and cleanup failures remain errors, and every exit stops/powers down the
started session and cancels outstanding transfers.
