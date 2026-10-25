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
 * - Open claims the interface but does not capture. Each capture consumes that
 *   USB session and releases the reader on exit. Subsequent captures reopen it;
 *   exclusive ownership is not retained between successive captures.
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

UAREU_API uareu_status UAREU_CALL uareu_reader_list_create(uareu_handle *output);
UAREU_API uareu_status UAREU_CALL uareu_reader_list_count(uareu_handle list, uint32_t *count);
/* Discovery JSON: schema_version, index, numeric vendor_id/product_id/
 * device_version, and nullable UTF-8 product/manufacturer/serial_number. */
UAREU_API uareu_status UAREU_CALL uareu_reader_list_get_json(uareu_handle list, uint32_t index, uint8_t *buffer, size_t capacity, size_t *required);
UAREU_API uareu_status UAREU_CALL uareu_reader_list_free(uareu_handle list);
UAREU_API uareu_status UAREU_CALL uareu_reader_open(uareu_handle list, uint32_t index, uareu_handle *output);
UAREU_API uareu_status UAREU_CALL uareu_reader_close(uareu_handle reader);

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

#ifdef __cplusplus
}
#endif
#endif
