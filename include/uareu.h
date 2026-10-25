/* SPDX-License-Identifier: GPL-3.0-or-later */
#ifndef UAREU_H
#define UAREU_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#  define UAREU_API __declspec(dllimport)
#  define UAREU_CALL __cdecl
#else
#  define UAREU_API
#  define UAREU_CALL
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define UAREU_ABI_VERSION 1u
typedef uint64_t uareu_handle;
typedef uint32_t uareu_status;
#define UAREU_OK                0u
#define UAREU_INVALID_ARGUMENT  1u
#define UAREU_INVALID_HANDLE    2u
#define UAREU_BUFFER_TOO_SMALL  3u
#define UAREU_BUSY              4u
#define UAREU_CANCELLED         5u
#define UAREU_DEVICE_ERROR      6u
#define UAREU_IMAGE_ERROR       7u
#define UAREU_PANIC             8u
#define UAREU_INTERNAL_ERROR    9u

/* ABI v1: four uint32_t fields, natural layout, total size 16 bytes. */
typedef struct uareu_image_info {
    uint32_t width;
    uint32_t height;
    uint32_t stride_bytes;
    uint32_t bits_per_pixel;
} uareu_image_info;

/* Ownership/concurrency:
 * - Handles are opaque, process-local, nonzero uint64_t IDs, never pointers.
 *   Free/close each successful output handle exactly once using its matching API.
 *   Invalid, stale, double-freed, and wrong-kind handles return INVALID_HANDLE.
 * - The DLL must remain loaded until every call completes and every handle is freed.
 * - All output pointers must be valid/aligned; buffers must have their declared
 *   capacities. Input/output ranges must not overlap and must remain valid for
 *   the whole call. Invalid addresses cannot be made safe by argument validation.
 * - Discovery lists/images are immutable and permit concurrent reads. A reader
 *   permits one capture at a time (another capture returns BUSY). Cancel and
 *   close may run concurrently with capture from another thread.
 * - Close invalidates the handle immediately and requests active cancellation;
 *   the capture call retains internal state until cleanup finishes. Wait for
 *   that call before unloading the DLL or exiting your worker.
 * - Releasing an image/list does not interrupt an already-entered read call.
 * - A discovery snapshot stays fixed; indices are meaningful only in that list.
 *   Reader handles survive freeing the list. Re-enumerate after reconnecting.
 * - Open claims the interface but does not capture. Default captures release
 *   that USB session on exit and reopen it next time. open_ex with
 *   UAREU_OPEN_PERSISTENT retains the interface between successful/cancelled
 *   operations; a detected disconnection discards the session for reopening.
 * - Windows requires WinUSB assigned to USB 05ba:000a. No vendor SDK is required.
 */

UAREU_API uint32_t UAREU_CALL uareu_abi_version(void);

/* UTF-8 JSON/error buffers include a trailing NUL in *required; pixels do not.
 * Probe with NULL/0, receiving BUFFER_TOO_SMALL and the required byte count,
 * allocate that many bytes, then call again. Too-small buffers are not modified.
 * required is mandatory, even during a probe. Never free a DLL-owned pointer:
 * all data is copied into your memory. Successful calls clear the current
 * thread's error; failing calls replace it. Read the error on the failing thread.
 * uareu_last_error itself never clears/replaces that error, including probes.
 */
UAREU_API uareu_status UAREU_CALL uareu_last_error(uint8_t *buffer, size_t capacity, size_t *required);
/* Additive ABI v1 query: UTF-8 JSON {schema_version,status,kind,stage,message}.
 * Like last_error, this never replaces/clears the failing thread's error.
 * Existing status values/return conventions are unchanged. In particular,
 * timeout/disconnect/USB failures still return DEVICE_ERROR; inspect kind.
 * kind distinguishes timeout, cancelled, disconnected, busy, permission_denied,
 * invalid_argument, unsupported, invalid_frame, invalid_response, device_failure,
 * transport and other, plus API errors such as invalid_handle/image_error.
 * stage is open/initialization/finger_wait/acquisition/decode/cleanup or null.
 * A backend-cancelled transfer may also be a backend timeout: kind remains
 * transport unless the protocol itself observed cancellation/deadline expiry.
 */
UAREU_API uareu_status UAREU_CALL uareu_last_error_json(uint8_t *buffer, size_t capacity, size_t *required);

UAREU_API uareu_status UAREU_CALL uareu_reader_list_create(uareu_handle *output);
UAREU_API uareu_status UAREU_CALL uareu_reader_list_count(uareu_handle list, uint32_t *count);
/* Discovery JSON: schema_version, index, numeric vendor_id/product_id/
 * device_version, and nullable UTF-8 product/manufacturer/serial_number. */
UAREU_API uareu_status UAREU_CALL uareu_reader_list_get_json(uareu_handle list, uint32_t index, uint8_t *buffer, size_t capacity, size_t *required);
UAREU_API uareu_status UAREU_CALL uareu_reader_list_free(uareu_handle list);
UAREU_API uareu_status UAREU_CALL uareu_reader_open(uareu_handle list, uint32_t index, uareu_handle *output);
/* Explicit opt-in persistent interface ownership. Existing open keeps its
 * original one-shot interface-release behavior. Close releases the session;
 * disconnect or transport failure may still make the device unusable. */
#define UAREU_OPEN_PERSISTENT 1u
UAREU_API uareu_status UAREU_CALL uareu_reader_open_ex(uareu_handle list, uint32_t index, uint32_t flags, uareu_handle *output);
UAREU_API uareu_status UAREU_CALL uareu_reader_close(uareu_handle reader);
/* Capabilities JSON schema 1 separates implemented SDK operations (booleans)
 * from device capabilities (unknown/supported/unsupported). Device flags and
 * resolutions remain unknown/null: this SDK has no authoritative hardware
 * capability query. SDK false means unimplemented, not hardware unsupported.
 * List query does not open/claim a device; reader query requires a live handle.
 */
UAREU_API uareu_status UAREU_CALL uareu_reader_list_get_capabilities_json(uareu_handle list, uint32_t index, uint8_t *buffer, size_t capacity, size_t *required);
UAREU_API uareu_status UAREU_CALL uareu_reader_get_capabilities_json(uareu_handle reader, uint8_t *buffer, size_t capacity, size_t *required);
/* Cached SDK lifecycle snapshot, no USB I/O and no physical-state probe.
 * JSON schema 1: source, sequence, operation, capture_active, interface_claimed,
 * finger_present, hardware_ready, calibration_required, connected, last_outcome;
 * DLL additionally exposes busy (capture lease) and cancellation_requested.
 * sequence is a local observation counter. Hardware readiness, connectivity,
 * calibration remain null. Finger presence is only a cached true observation
 * during acquisition; idle/initializing/waiting/finished states return null.
 * Completed capture does not imply biometric quality. Closed/stale handles fail.
 * Snapshots can change between buffer probes; retry BUFFER_TOO_SMALL and use
 * the actual required count after success (the JSON may also shrink).
 */
UAREU_API uareu_status UAREU_CALL uareu_reader_get_status_json(uareu_handle reader, uint8_t *buffer, size_t capacity, size_t *required);
/* Explicit bounded status-register observation: timestamp, raw register and
 * low-power flag. Readiness/calibration/DPI remain unknown. Requires idle
 * reader; concurrent capture returns BUSY. Output is an owned data handle. */
UAREU_API uareu_status UAREU_CALL uareu_reader_probe(uareu_handle reader, uareu_handle *observation);
/* Stop/low-power/queue rearm only, NOT vendor reset or calibration. */
UAREU_API uareu_status UAREU_CALL uareu_reader_recover(uareu_handle reader);

/* Blocking single capture, best run on a caller-owned worker thread.
 * timeout_ms: 1..3600000; rotate180: 0 or 1. Output is zero on failure.
 * Cancellation applies only to an active capture; idle cancel is a no-op.
 * Timeout covers protocol acquisition, not OS open/configure/close operations.
 * There is an additional bounded USB cleanup allowance. No signal handler is
 * installed by the DLL. Device errors include timeout, unplug, and USB errors;
 * use the thread-local text for detail. Rust panics are contained as UAREU_PANIC
 * when built with Rust's default unwind strategy.
 */
UAREU_API uareu_status UAREU_CALL uareu_capture(uareu_handle reader, uint32_t timeout_ms, uint32_t rotate180, uareu_handle *output);
/* Optional synchronous notifications on the capture thread, matching Rust/CLI
 * CaptureEvent. Callback/context must remain valid until capture returns.
 * Callback must not throw/unwind across the ABI, unload the DLL, or wait for
 * this capture to finish. It may request cancellation. NULL disables callbacks.
 * This changes only phase reporting; pixels/options follow uareu_capture.
 */
#define UAREU_PHASE_INITIALIZING       1u
#define UAREU_PHASE_WAITING_FOR_FINGER 2u
#define UAREU_PHASE_WAITING_FOR_REMOVAL 3u
#define UAREU_PHASE_CAPTURING          4u
typedef void (UAREU_CALL *uareu_capture_callback)(uint32_t phase, void *context);
UAREU_API uareu_status UAREU_CALL uareu_capture_with_callback(uareu_handle reader, uint32_t timeout_ms, uint32_t rotate180, uareu_capture_callback callback, void *context, uareu_handle *output);
UAREU_API uareu_status UAREU_CALL uareu_cancel(uareu_handle reader);
/* Repeated placements, NOT video streaming. Caller owns each image handle
 * upon callback entry (including stop/error) and must free it exactly once.
 * Return 0 to continue, 1 to stop. Other values fail INVALID_ARGUMENT.
 * Phase/image callbacks are synchronous and must not unwind or unload DLL.
 * Cancellation returns CANCELLED; legacy Rust APIs still stop gracefully OK. */
typedef uint32_t (UAREU_CALL *uareu_image_callback)(uareu_handle image, void *context);
UAREU_API uareu_status UAREU_CALL uareu_capture_continuous(uareu_handle reader, uint32_t timeout_ms, uint32_t rotate180, uareu_image_callback image, uareu_capture_callback phase, void *context);
/* Owned async job: start reserves the reader before returning; worker performs
 * capture. No foreign callbacks run on the worker. Wait copies errors onto its
 * calling thread. completed=0 means pending; completed=1 consumes image/error
 * once. Free cancels AND JOINS worker; free every job before unloading DLL.
 * Task cancellation and reader cancellation target the same operation. */
UAREU_API uareu_status UAREU_CALL uareu_capture_start(uareu_handle reader, uint32_t timeout_ms, uint32_t rotate180, uareu_handle *task);
UAREU_API uareu_status UAREU_CALL uareu_task_wait(uareu_handle task, uint32_t timeout_ms, uareu_handle *image, uint32_t *completed);
UAREU_API uareu_status UAREU_CALL uareu_task_cancel(uareu_handle task);
UAREU_API uareu_status UAREU_CALL uareu_task_get_status_json(uareu_handle task, uint8_t *buffer, size_t capacity, size_t *required);
UAREU_API uareu_status UAREU_CALL uareu_task_free(uareu_handle task);

UAREU_API uareu_status UAREU_CALL uareu_image_get_info(uareu_handle image, uareu_image_info *info, size_t info_size);
UAREU_API uareu_status UAREU_CALL uareu_image_get_pixels(uareu_handle image, uint8_t *buffer, size_t capacity, size_t *required);
/* JSON schema 1 is the same metadata/diagnostics report as CLI `quality`.
 * Capture resolution stays unknown. Quality is not_evaluated; no NFIQ engine
 * is invoked and no biometric acceptance/liveness claim is made.
 */
UAREU_API uareu_status UAREU_CALL uareu_image_get_report_json(uareu_handle image, uint8_t *buffer, size_t capacity, size_t *required);
/* Paths are explicit-length UTF-8, excluding any terminator; embedded NUL fails.
 * Save never overwrites. Load is offline and requires 8-bit grayscale PNG.
 */
UAREU_API uareu_status UAREU_CALL uareu_image_save_png(uareu_handle image, const uint8_t *path, size_t path_length);
UAREU_API uareu_status UAREU_CALL uareu_image_load_png(const uint8_t *path, size_t path_length, uareu_handle *output);
UAREU_API uareu_status UAREU_CALL uareu_image_free(uareu_handle image);
/* JSON assessment config: engine_path/model_path optional, timeout_ms 1..300000,
 * ppi optional EXPLICIT caller declaration. No declaration means retain source
 * metadata. Engine executes once; output data handle owns immutable JSON.
 * Unknown resolution returns a report with unsupported_input, never guessed DPI. */
UAREU_API uareu_status UAREU_CALL uareu_image_assess(uareu_handle image, const uint8_t *config, size_t length, uareu_handle *report);
/* Display-only percentile contrast stretch. Creates new image/provenance;
 * original pixels stay intact. Not vendor enhancement, PIV, or dewarping. */
UAREU_API uareu_status UAREU_CALL uareu_image_enhance(uareu_handle image, uareu_handle *output);
UAREU_API uareu_status UAREU_CALL uareu_data_get_json(uareu_handle data, uint8_t *buffer, size_t capacity, size_t *required);
UAREU_API uareu_status UAREU_CALL uareu_data_free(uareu_handle data);
UAREU_API uareu_status UAREU_CALL uareu_data_from_json(const uint8_t *input, size_t length, uareu_handle *data);
UAREU_API uareu_status UAREU_CALL uareu_image_from_pixels(uint32_t width, uint32_t height, const uint8_t *pixels, size_t length, const uint8_t *metadata, size_t metadata_length, uareu_handle *image);
/* Optional external NBIS tools. Config: encoder, decoder, timeout_ms, bitrate.
 * Encoding requires explicit 500 PPI. Data is a JSON byte array handle.
 * Decode preserves NISTCOM declared scale, not proof of calibration.
 * WSQ is lossy; no FBI encoder certification is claimed. */
UAREU_API uareu_status UAREU_CALL uareu_image_encode_wsq(uareu_handle image, const uint8_t *config, size_t length, uareu_handle *data);
UAREU_API uareu_status UAREU_CALL uareu_image_decode_wsq(uareu_handle data, const uint8_t *config, size_t length, uareu_handle *image);
/* Single-view ISO 19794-4:2005 FIR/010 raw 8-bit subset. Config requires explicit
 * capture_device_id, acquisition_level, scale_units (1=PPI,2=PPCM),
 * scan_resolution_x/y, finger_position, quality, impression. Source image
 * resolution must be known and representable. No scan scale/quality/default
 * certification is guessed. Data handles contain JSON byte arrays. */
UAREU_API uareu_status UAREU_CALL uareu_image_encode_record(uareu_handle image, const uint8_t *config, size_t length, uareu_handle *data);
UAREU_API uareu_status UAREU_CALL uareu_image_decode_record(uareu_handle data, uareu_handle *image);
/* Optional local SourceAFIS helper config: executable, timeout_ms, optional
 * explicit ppi for extraction. Requests use operation:
 * extract {image}; compare {probe,candidate,threshold}; identify
 * {probe,candidates:[handles],threshold,maximum}; enroll {samples:[handles],
 * threshold}; verify_enrollment {probe,enrollment,threshold}; export_template
 * {template,format,quality,impression}; import_template {data,format}.
 * iso19794_2_2005/ansi378_2004 support one view/no extensions only. Quality and
 * impression are explicit caller declarations. Import data is a data handle
 * holding the binary template as a JSON byte array.
 * Templates/enrollment/results are owned JSON data handles. Input handles are
 * borrowed. Positive finite thresholds are mandatory, application-calibrated.
 * Multi-template enrollment is not vendor fusion. Scores/format are NOT vendor
 * scores. Native caches are version bound; ANSI/ISO conversions are limited
 * and quantize coordinates/angles. Engine build identity is recorded/checked. */
UAREU_API uareu_status UAREU_CALL uareu_recognition_execute(const uint8_t *config, size_t config_length, const uint8_t *request, size_t request_length, uareu_handle *output);

#ifdef __cplusplus
}
#endif
#endif
