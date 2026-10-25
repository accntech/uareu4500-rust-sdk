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
 * sequence is a local observation counter. Hardware readiness and
 * calibration remain null; connectivity can reflect an OS USB hotplug event.
 * Finger presence is only a cached true observation
 * during acquisition; idle/initializing/waiting/finished states return null.
 * Completed capture does not imply biometric quality. Closed/stale handles fail.
 * Snapshots can change between buffer probes; retry BUFFER_TOO_SMALL and use
 * the actual required count after success (the JSON may also shrink).
 */
UAREU_API uareu_status UAREU_CALL uareu_reader_get_status_json(uareu_handle reader, uint8_t *buffer, size_t capacity, size_t *required);
/* Cached observation wait, no USB polling. timeout_ms 0..300000; sequence is
 * local to this reader (from get_status_json or an earlier wait). Returns OK,
 * completed=0/data=0 on timeout; OK/completed=1 with an owned immutable JSON
 * ReaderStatus snapshot on a newer sequence. No ReaderState lock is held while
 * waiting; closing wakes in-progress waits, which can return operation=closed.
 * New calls on a closed/stale handle return INVALID_HANDLE. Free data once.
 * Snapshot schema: {schema_version:u32,source:string,sequence:u64,
 * operation:string,capture_active:bool,interface_claimed:bool,
 * finger_present:bool|null,hardware_ready:bool|null,
 * calibration_required:bool|null,connected:bool|null,
 * last_outcome:{success:bool,failure:CaptureFailure|null}|null,
 * last_hardware_observation:{source:string,observed_unix_ms:u64|null,
 * connected_at_observation:bool,hardware_status_register:u8,low_power:bool,
 * hardware_ready:bool|null,calibration_required:bool|null}|null,
 * last_probe_failure:CaptureFailure|null,hotplug_monitoring:bool}.
 * CaptureFailure={kind:string,stage:string|null,message:string,
 * cleanup:CaptureFailure|null}, recursively, as in last_error_json.
 * This immutable snapshot excludes get_status_json's added busy,
 * persistent_session, cancellation_requested fields. Outputs zero on failure. */
UAREU_API uareu_status UAREU_CALL uareu_reader_wait_status_json(uareu_handle reader, uint64_t after_sequence, uint32_t timeout_ms, uint32_t *completed, uareu_handle *data);
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
/* Alternative async completion delivery on the capture worker, exactly once
 * after acquisition/cleanup. A successful start publishes *task before enabling
 * delivery; callback can run before start returns. Failed start never delivers.
 * Callback owns image on OK, or a JSON error-report data handle on failure
 * (report may be zero if allocation fails). Free each with matching image/data
 * API. No SDK lock is held across the callback. Do not unwind or unload DLL.
 * Result is consumed by callback delivery; task_wait cannot consume it again.
 * Poll task status/cancel remain available. Free joins callback completion;
 * deadlines do not bound callback code. Free from its own callback returns BUSY
 * without removing the task; free it from another thread after delivery.
 * Keep callback/context valid until task_free returns. This is our cdecl ABI,
 * not a vendor DPFPDD callback struct or __stdcall compatibility alias. */
typedef void (UAREU_CALL *uareu_completion_callback)(uareu_status status, uareu_handle image, uareu_handle error_report, void *context);
UAREU_API uareu_status UAREU_CALL uareu_capture_start_with_completion(uareu_handle reader, uint32_t timeout_ms, uint32_t rotate180, uareu_completion_callback callback, void *context, uareu_handle *task);
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
/* Score one zero-based image in a MultiImageRecord JSON data handle. Config:
 * optional engine_path/model_path, timeout_ms 1..300000 (default 10000).
 * ppi overrides are rejected: resolution comes from the record declaration.
 * Opaque compression, invalid index, or invalid declarations return an owned
 * {status:"unsupported_input",reason:string} outcome, without codec execution.
 * No engine gives {status:"not_evaluated"} for a valid raw view.
 * Other outcome JSON statuses: engine_unavailable/evaluation_failed with reason,
 * or {status:"scored",engine:{name:string,version:string,
 * executable_sha256:string,model_source:string,model_info_sha256:string|null,
 * model_data_sha256:string|null},score:u8 (0..100),input_region:{x:size_t,
 * y:size_t,width:size_t,height:size_t},evaluation_ppi:u32,
 * actionable_feedback:{string:number}}.
 * Outcome status must be checked even when the ABI function returns OK.
 * Inputs borrowed; output owned, freed with uareu_data_free; zero on failure. */
UAREU_API uareu_status UAREU_CALL uareu_image_assess_record_view(uareu_handle record, size_t view, const uint8_t *config, size_t length, uareu_handle *result);

/* Measured WSQ target search. Config JSON: {encoder:string,decoder:string,
 * target_bytes:1..4194304,max_encodes?:1..32 (default 8),
 * timeout_ms?:1..300000 (default 30000; shared search/per-process budget)}.
 * No bitrate field: search tests supported rates 0.1..2.25 bits/pixel.
 * Output data JSON: {bytes:[u8],bitrate:number,encodes:integer,
 * status:"met"|"unattainable"|"encode_limit"|"deadline"|"codec_failure",
 * failure:string|null,target_bytes:integer}. OK means a measured candidate
 * exists; only status="met" promises its length fits this target. Best valid
 * candidate survives later failures. No successful candidate => IMAGE_ERROR.
 * Requires explicit 500 PPI. External NBIS tools are caller supplied. */
UAREU_API uareu_status UAREU_CALL uareu_image_encode_wsq_target(uareu_handle image, const uint8_t *config, size_t length, uareu_handle *result);

/* Additive interchange APIs. All inputs are BORROWED immutable data handles
 * from uareu_data_from_json; successful outputs are OWNED data handles, freed
 * once with uareu_data_free. Encode returns a JSON array of binary bytes;
 * decode returns the schema below. Output is zero on failure. JSON input
 * import is bounded to 16 MiB; existing status codes remain unchanged.
 * These are independent SDK APIs, never dpfpdd/dpfj vendor ABI aliases. */
#define UAREU_TEMPLATE_ISO_19794_2_2005 UINT32_C(0)
#define UAREU_TEMPLATE_ANSI_378_2004 UINT32_C(1)
/* MultiViewRecord JSON (all fields required):
 * {width:u16,height:u16,pixels_per_cm_x:u16,pixels_per_cm_y:u16,
 *  vendor_id:u16,subformat:u16,device_info:u16,views:[
 *   {finger_position:u8,view_number:u8,impression:u8,quality:u8,minutiae:[
 *    {x:u16,y:u16,kind:u8,direction_clockwise:number,quality:u8}]}]}.
 * Explicit format must be one of the constants above. Geometry and scale >0;
 * ISO scale >=99 pixels/cm. ANSI requires vendor_id>0; ISO cannot store vendor
 * or subformat (decoded as zero). Reserved device bits 0x7000 must be zero.
 * At most 176 views, positions 0..10, view numbers 0..15 consecutive from zero
 * per finger in record order, at most 255 minutiae/view. Impression 0..3 or 8
 * (ANSI also 9), quality 0..100; minutia kind 0..2, x/y inside geometry and
 * <16384; finite clockwise direction in [0,2*pi), quantized by binary format.
 * Extensions are rejected. This is transport/editable JSON, not extraction,
 * recognition, biometric quality measurement, or certified interoperability. */
UAREU_API uareu_status UAREU_CALL uareu_template_encode_views(uareu_handle record, uint32_t format, uareu_handle *data);
UAREU_API uareu_status UAREU_CALL uareu_template_decode_views(uareu_handle data, uint32_t format, uareu_handle *record);
/* Additive opaque extension transport: JSON {record:MultiViewRecord,
 * extensions:[[{type_code:u16,data:[u8]}]]}; one list per record view.
 * Reserved types and 65535-byte aggregate extension limits are validated.
 * Payload semantics are uninterpreted and must match the selected format.
 * Existing no-extension APIs still reject extension-bearing binary records.
 * Inputs borrowed; outputs owned JSON data handles, zero on failure. */
UAREU_API uareu_status UAREU_CALL uareu_template_encode_extended(uareu_handle record, uint32_t format, uareu_handle *data);
UAREU_API uareu_status UAREU_CALL uareu_template_decode_extended(uareu_handle data, uint32_t format, uareu_handle *record);

/* Optional external OpenJPEG lossless grayscale raw J2K codec. Config requires
 * encoder:string,decoder:string; optional timeout_ms 1..300000 (default30000).
 * Single-tile unsigned8-bit, one component; <=1MiB pixels, <=4MiB codestream.
 * JP2 boxes, color, subsampling and high-bit-depth input are unsupported.
 * Encode returns owned JSON byte-array data; decode returns an owned image.
 * Raw codestreams carry no physical DPI: decode resolution remains unknown.
 * Inputs borrowed; output zero on error; free data/image with matching APIs. */
UAREU_API uareu_status UAREU_CALL uareu_image_encode_jpeg2000(uareu_handle image, const uint8_t *config, size_t length, uareu_handle *data);
UAREU_API uareu_status UAREU_CALL uareu_image_decode_jpeg2000(uareu_handle data, const uint8_t *config, size_t length, uareu_handle *image);

/* Separate legacy NBIS5 NFIQ1 assessment. Config requires engine_path:string;
 * optional timeout_ms 1..300000 (default10000), ppi caller declaration.
 * Explicit500PPI needed; no resampling; source image/metadata stay unchanged.
 * Owned JSON outcome status: scored/unsupported_input/engine_unavailable/
 * evaluation_failed. Scored score1..5 LOWER IS BETTER, never an NFIQ2 score.
 * ABI OK indicates an outcome exists; inspect its status before using score.
 * Output zero on ABI error; free successful result with uareu_data_free. */
UAREU_API uareu_status UAREU_CALL uareu_image_assess_nfiq1(uareu_handle image, const uint8_t *config, size_t length, uareu_handle *outcome);
/* MultiImageRecord JSON (all fields required), ISO 19794-4:2005 FIR/010 only:
 * {capture_device_id:u16,acquisition_level:u16,scale_units:u8,
 *  scan_resolution_x:u16,scan_resolution_y:u16,image_resolution_x:u16,
 *  image_resolution_y:u16,compression:"raw8_bit"|"opaque_wsq"|
 *  "opaque_jpeg2000",images:[{finger_position:u8,view_count:u8,
 *  view_number:u8,quality:u8,impression:u8,width:u16,height:u16,data:[u8]}]}.
 * 1..255 images; device<4096; scale_units 1=PPI/2=PPCM; image resolutions >0
 * and <=scan resolutions. Positions 0..10; quality 0..100; impression 0..3/7..9;
 * dimensions>0. For each finger view_count matches its image count and unique
 * view_number covers 1..view_count. Raw8 data length=width*height <=1 MiB;
 * opaque compressed data nonempty <=4 MiB/image; binary record <=32 MiB.
 * Opaque data is transported unchanged: compressed dimensions, bit depth,
 * image quality, WSQ/JPEG2000 validity are NOT verified or decompressed.
 * Header resolution and acquisition values remain caller declarations. */
UAREU_API uareu_status UAREU_CALL uareu_image_encode_images(uareu_handle record, uareu_handle *data);
UAREU_API uareu_status UAREU_CALL uareu_image_decode_images(uareu_handle data, uareu_handle *record);
/* Independent ANSI INCITS381:2004 FIR/010 raw8 transport. Record JSON:
 * {cbeff_product_id:u32,appendix_f_compliant:bool,record:MultiImageRecord}.
 * Product owner high16bits must be nonzero. Acquisition levels10/20/30/31/40/41;
 * scan resolution<=1000 units; quality1..100 or254(undefined); impression0..3/7/8.
 * Existing common geometry/scale/view limits apply. Compression must raw8_bit.
 * Compliance/product/header scale remain declarations, not measured facts.
 * Encode/decode return owned JSON data; view selects a zero-based index and
 * returns an owned image with declared scale. Inputs borrowed; output0 on error.
 * These are separate APIs: passing ANSI bytes to ISO APIs does not convert them.
 * Other ANSI editions/compression/slap positions remain unsupported. */
UAREU_API uareu_status UAREU_CALL uareu_image_encode_ansi381(uareu_handle record, uareu_handle *data);
UAREU_API uareu_status UAREU_CALL uareu_image_decode_ansi381(uareu_handle data, uareu_handle *record);
UAREU_API uareu_status UAREU_CALL uareu_image_ansi381_view(uareu_handle record, size_t view, uareu_handle *image);

/* Single-view ISO 19794-4:2005 FIR/010 raw 8-bit subset. Config requires explicit
 * capture_device_id, acquisition_level, scale_units (1=PPI,2=PPCM),
 * scan_resolution_x/y, finger_position, quality, impression. Source image
 * resolution must be known and representable. No scan scale/quality/default
 * certification is guessed. Data handles contain JSON byte arrays. */
UAREU_API uareu_status UAREU_CALL uareu_image_encode_record(uareu_handle image, const uint8_t *config, size_t length, uareu_handle *data);
UAREU_API uareu_status UAREU_CALL uareu_image_decode_record(uareu_handle data, uareu_handle *image);
/* Host flat-field correction using explicit same-size dark and bright reference
 * images. minimum_response must be finite and in (0,255]. Profile is an owned
 * JSON data handle; apply returns an owned image and preserves source metadata
 * with profile-hash provenance. Inputs are borrowed; output is zero on failure.
 * This does not calibrate reader hardware or establish PIV compliance. */
UAREU_API uareu_status UAREU_CALL uareu_image_flat_field_profile(uareu_handle dark, uareu_handle bright, double minimum_response, uareu_handle *profile);
UAREU_API uareu_status UAREU_CALL uareu_image_apply_flat_field(uareu_handle image, uareu_handle profile, uareu_handle *output);

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
