// SPDX-License-Identifier: GPL-3.0-or-later
//! Version 1 C ABI. See `include/uareu.h` for ownership and concurrency rules.
//! Existing Rust capture APIs do not use this registry or change their behavior.
use crate::{
    error::{CaptureError, CaptureErrorKind, CaptureFailure, CaptureStage},
    metadata::CapturedImage,
    protocol::{CaptureEvent, CaptureOptions},
    quality,
    reader::{ReaderCapabilities, ReaderMonitor},
    task::CaptureTask,
    usb::{self, Reader},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub const OK: u32 = 0;
pub const INVALID_ARGUMENT: u32 = 1;
pub const INVALID_HANDLE: u32 = 2;
pub const BUFFER_TOO_SMALL: u32 = 3;
pub const BUSY: u32 = 4;
pub const CANCELLED: u32 = 5;
pub const DEVICE_ERROR: u32 = 6;
pub const IMAGE_ERROR: u32 = 7;
pub const PANIC: u32 = 8;
pub const INTERNAL_ERROR: u32 = 9;

type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug)]
struct Error(u32, String, Option<CaptureFailure>);
fn error(code: u32, message: impl Into<String>) -> Error {
    Error(code, message.into(), None)
}
fn mapped(code: u32, cause: anyhow::Error) -> Error {
    let failure = (cause.is::<CaptureError>()
        || cause.is::<crate::task::TaskError>()
        || cause.is::<crate::error::CleanupFailure>())
    .then(|| CaptureFailure::from_error(&cause));
    Error(code, format!("{cause:#}"), failure)
}

#[derive(Clone, Default, serde::Serialize)]
struct ErrorReport {
    schema_version: u32,
    status: u32,
    kind: String,
    stage: Option<CaptureStage>,
    message: String,
    cleanup: Option<Box<CaptureFailure>>,
}
impl ErrorReport {
    fn from_failure(failure: Error) -> Self {
        let generic = match failure.0 {
            INVALID_ARGUMENT => "invalid_argument",
            INVALID_HANDLE => "invalid_handle",
            BUFFER_TOO_SMALL => "buffer_too_small",
            BUSY => "busy",
            CANCELLED => "cancelled",
            DEVICE_ERROR => "device_error",
            IMAGE_ERROR => "image_error",
            PANIC => "panic",
            _ => "internal_error",
        };
        Self {
            schema_version: 1,
            status: failure.0,
            kind: failure.2.as_ref().map_or_else(
                || generic.into(),
                |detail| {
                    serde_json::to_value(detail.kind)
                        .expect("enum serialization")
                        .as_str()
                        .unwrap()
                        .into()
                },
            ),
            stage: failure.2.as_ref().and_then(|detail| detail.stage),
            cleanup: failure.2.and_then(|detail| detail.cleanup),
            message: failure.1,
        }
    }
    fn cleared() -> Self {
        Self {
            schema_version: 1,
            kind: "none".into(),
            ..Self::default()
        }
    }
}
thread_local! { static LAST_ERROR: RefCell<ErrorReport> = RefCell::new(ErrorReport::cleared()); }

fn boundary(operation: impl FnOnce() -> Result<()>) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(operation));
    let failure = match result {
        Ok(Ok(())) => {
            LAST_ERROR.with(|last| *last.borrow_mut() = ErrorReport::cleared());
            return OK;
        }
        Ok(Err(error)) => error,
        Err(_) => error(PANIC, "Rust panic contained at the DLL boundary"),
    };
    let status = failure.0;
    LAST_ERROR.with(|last| *last.borrow_mut() = ErrorReport::from_failure(failure));
    status
}

struct ReaderState {
    closed: bool,
    idle_reader: Option<Reader>,
    active: Option<Arc<AtomicBool>>,
    monitor: ReaderMonitor,
    persistent: bool,
}
impl ReaderState {
    fn cancel(&self) -> Result<()> {
        if self.closed {
            return Err(error(INVALID_HANDLE, "reader is closed"));
        }
        if let Some(cancel) = &self.active {
            cancel.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
    fn close(&mut self) {
        if let Some(cancel) = &self.active {
            cancel.store(true, Ordering::Relaxed);
        }
        self.closed = true;
        self.idle_reader.take();
        self.monitor.close();
    }
}
struct ReaderContext {
    info: nusb::DeviceInfo,
    state: Arc<Mutex<ReaderState>>,
}
struct ImageEntry {
    captured: CapturedImage,
    report: Vec<u8>,
}
#[derive(Clone)]
enum Object {
    List(Arc<Vec<nusb::DeviceInfo>>),
    Reader(Arc<ReaderContext>),
    Image(Arc<ImageEntry>),
    Task(Arc<CaptureTask>),
    Data(Arc<Vec<u8>>),
}
struct Registry {
    next: u64,
    objects: HashMap<u64, Object>,
}
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            next: 1,
            objects: HashMap::new(),
        })
    })
}
fn insert(object: Object) -> Result<u64> {
    let mut registry = registry()
        .lock()
        .map_err(|_| error(INTERNAL_ERROR, "handle registry is poisoned"))?;
    let id = registry.next;
    registry.next = id
        .checked_add(1)
        .ok_or_else(|| error(INTERNAL_ERROR, "handle space exhausted"))?;
    registry.objects.insert(id, object);
    Ok(id)
}
fn lookup(id: u64) -> Result<Object> {
    registry()
        .lock()
        .map_err(|_| error(INTERNAL_ERROR, "handle registry is poisoned"))?
        .objects
        .get(&id)
        .cloned()
        .ok_or_else(|| error(INVALID_HANDLE, "unknown or released handle"))
}
fn reader(id: u64) -> Result<Arc<ReaderContext>> {
    match lookup(id)? {
        Object::Reader(reader) => Ok(reader),
        _ => Err(error(INVALID_HANDLE, "expected reader handle")),
    }
}
fn list(id: u64) -> Result<Arc<Vec<nusb::DeviceInfo>>> {
    match lookup(id)? {
        Object::List(list) => Ok(list),
        _ => Err(error(INVALID_HANDLE, "expected reader-list handle")),
    }
}
fn image(id: u64) -> Result<Arc<ImageEntry>> {
    match lookup(id)? {
        Object::Image(image) => Ok(image),
        _ => Err(error(INVALID_HANDLE, "expected image handle")),
    }
}
fn release(id: u64, expected: fn(&Object) -> bool) -> Result<Object> {
    let mut registry = registry()
        .lock()
        .map_err(|_| error(INTERNAL_ERROR, "handle registry is poisoned"))?;
    match registry.objects.get(&id) {
        Some(object) if expected(object) => Ok(registry
            .objects
            .remove(&id)
            .expect("entry checked while locked")),
        _ => Err(error(
            INVALID_HANDLE,
            "unknown, released, or wrong-kind handle",
        )),
    }
}

struct CaptureGuard {
    state: Arc<Mutex<ReaderState>>,
    cancel: Arc<AtomicBool>,
}
impl CaptureGuard {
    fn begin(shared: Arc<Mutex<ReaderState>>) -> Result<(Self, Option<Reader>)> {
        Self::begin_mode(shared, true)
    }
    fn begin_mode(
        shared: Arc<Mutex<ReaderState>>,
        capture: bool,
    ) -> Result<(Self, Option<Reader>)> {
        let mut state = shared
            .lock()
            .map_err(|_| error(INTERNAL_ERROR, "reader state is poisoned"))?;
        if state.closed {
            return Err(error(INVALID_HANDLE, "reader is closed"));
        }
        if state.active.is_some() {
            return Err(error(BUSY, "a capture is already active on this reader"));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        state.active = Some(Arc::clone(&cancel));
        if capture {
            state.monitor.begin();
        } else {
            state.monitor.begin_probe();
        }
        let idle_reader = state.idle_reader.take();
        drop(state);
        Ok((
            Self {
                state: shared,
                cancel,
            },
            idle_reader,
        ))
    }
}
impl Drop for CaptureGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.active = None;
            let snapshot = state.monitor.snapshot();
            if snapshot.capture_active
                || snapshot.operation == crate::reader::ReaderOperation::Probing
            {
                let failure = CaptureFailure {
                    kind: if std::thread::panicking() {
                        CaptureErrorKind::Panic
                    } else {
                        CaptureErrorKind::Other
                    },
                    stage: None,
                    message: "reader operation aborted before completion".into(),
                    cleanup: None,
                };
                if snapshot.capture_active {
                    state.monitor.finish(Some(failure));
                } else {
                    state.monitor.finish_probe(Err(failure));
                }
            }
        }
    }
}
impl CaptureGuard {
    fn retain_mode(&self, device: Reader, persistent_only: bool) {
        if let Ok(mut state) = self.state.lock() {
            if (!persistent_only || state.persistent) && !state.closed {
                state.idle_reader = Some(device);
            }
        }
    }
    fn retain_after<T>(&self, device: Reader, result: &anyhow::Result<T>, persistent_only: bool) {
        if !result.as_ref().err().is_some_and(is_disconnected) {
            self.retain_mode(device, persistent_only);
        }
    }
}
fn is_disconnected(cause: &anyhow::Error) -> bool {
    fn contains(failure: &CaptureFailure) -> bool {
        failure.kind == CaptureErrorKind::Disconnected
            || failure.cleanup.as_deref().is_some_and(contains)
    }
    contains(&CaptureFailure::from_error(cause))
}
/// Explicit hardware-register observation; never guesses readiness/calibration/DPI.
/// # Safety
/// output must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_probe(handle: u64, output: *mut u64) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let context = reader(handle)?;
        let (guard, idle) = CaptureGuard::begin_mode(context.state.clone(), false)?;
        let mut device = open_operation_reader(&context, idle, true).map_err(capture_error)?;
        let result = device.probe_status();
        guard.retain_after(device, &result, false);
        let id = data(&result.map_err(capture_error)?)?;
        unsafe { write(output, id) }
    })
}
/// Known stop/low-power/queue recovery, distinct from vendor reset/calibration.
#[unsafe(no_mangle)]
pub extern "C" fn uareu_reader_recover(handle: u64) -> u32 {
    boundary(|| {
        let context = reader(handle)?;
        let (guard, idle) = CaptureGuard::begin_mode(context.state.clone(), false)?;
        let mut device = open_operation_reader(&context, idle, true).map_err(capture_error)?;
        let result = device.recover_interface();
        guard.retain_after(device, &result, false);
        result.map_err(capture_error)
    })
}
fn open_operation_reader(
    context: &ReaderContext,
    idle: Option<Reader>,
    probing: bool,
) -> anyhow::Result<Reader> {
    if let Some(reader) = idle {
        return Ok(reader);
    }
    let monitor = context
        .state
        .lock()
        .map_err(|_| anyhow::anyhow!("reader state is poisoned"))?
        .monitor
        .clone();
    match Reader::open(&context.info) {
        Ok(reader) => Ok(reader.with_monitor(monitor)),
        Err(cause) => {
            let failure = CaptureFailure::from_error(&cause);
            if probing {
                monitor.finish_probe(Err(failure));
            } else {
                monitor.finish(Some(failure));
            }
            Err(cause)
        }
    }
}
fn capture_once(
    context: Arc<ReaderContext>,
    guard: CaptureGuard,
    idle: Option<Reader>,
    options: CaptureOptions,
    event: impl FnMut(CaptureEvent),
) -> anyhow::Result<CapturedImage> {
    let mut device = open_operation_reader(&context, idle, false)?;
    let result = device.capture_next(options, &guard.cancel, event);
    guard.retain_after(device, &result, true);
    result
}
fn capture_error(cause: anyhow::Error) -> Error {
    if cause.downcast_ref::<crate::task::TaskError>()
        == Some(&crate::task::TaskError::WorkerPanicked)
    {
        return mapped(PANIC, cause);
    }
    mapped(
        if cause
            .downcast_ref::<CaptureError>()
            .is_some_and(|e| e.kind == CaptureErrorKind::Cancelled)
        {
            CANCELLED
        } else {
            DEVICE_ERROR
        },
        cause,
    )
}
fn task(handle: u64) -> Result<Arc<CaptureTask>> {
    match lookup(handle)? {
        Object::Task(task) => Ok(task),
        _ => Err(error(INVALID_HANDLE, "expected task handle")),
    }
}
fn register_image(captured: CapturedImage) -> Result<u64> {
    let report = quality::assess(&captured, None).map_err(|cause| mapped(IMAGE_ERROR, cause))?;
    let report =
        serde_json::to_vec(&report).map_err(|cause| error(IMAGE_ERROR, cause.to_string()))?;
    insert(Object::Image(Arc::new(ImageEntry { captured, report })))
}

fn valid_output<T>(pointer: *mut T) -> Result<()> {
    if pointer.is_null() || !pointer.is_aligned() {
        Err(error(
            INVALID_ARGUMENT,
            "output pointer is null or misaligned",
        ))
    } else {
        Ok(())
    }
}
// All raw memory is caller-owned. Exports validate null/alignment/length, but
// foreign callers must still provide valid, non-overlapping readable/writable memory.
unsafe fn write<T>(pointer: *mut T, value: T) -> Result<()> {
    valid_output(pointer)?;
    unsafe {
        pointer.write(value);
    }
    Ok(())
}
unsafe fn copy_buffer(
    bytes: &[u8],
    nul: bool,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> Result<()> {
    let length = bytes
        .len()
        .checked_add(usize::from(nul))
        .ok_or_else(|| error(INTERNAL_ERROR, "buffer length overflow"))?;
    unsafe {
        write(required, length)?;
    }
    if capacity > isize::MAX as usize || (capacity > 0 && buffer.is_null()) {
        return Err(error(
            INVALID_ARGUMENT,
            "invalid buffer pointer or capacity",
        ));
    }
    if capacity < length {
        return Err(error(
            BUFFER_TOO_SMALL,
            "buffer is too small; use the required byte count",
        ));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
        if nul {
            buffer.add(bytes.len()).write(0);
        }
    }
    Ok(())
}
unsafe fn utf8_path<'a>(bytes: *const u8, length: usize) -> Result<&'a Path> {
    if bytes.is_null() || length == 0 || length > 65_536 {
        return Err(error(
            INVALID_ARGUMENT,
            "path must contain 1..65536 UTF-8 bytes",
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| error(INVALID_ARGUMENT, "path is not valid UTF-8"))?;
    if text.contains('\0') {
        return Err(error(INVALID_ARGUMENT, "path contains an embedded NUL"));
    }
    Ok(Path::new(text))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub bits_per_pixel: u32,
}

#[unsafe(no_mangle)]
pub extern "C" fn uareu_abi_version() -> u32 {
    1
}

/// Obtain the calling thread's error. Probing/copying never replaces that error.
/// # Safety
/// `required` must be writable; `buffer` must hold `capacity` writable bytes or be null with zero capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_last_error(
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    match catch_unwind(AssertUnwindSafe(|| {
        let message = LAST_ERROR.with(|last| last.borrow().message.clone());
        unsafe { copy_buffer(message.as_bytes(), true, buffer, capacity, required) }
    })) {
        Ok(Ok(())) => OK,
        Ok(Err(error)) => error.0,
        Err(_) => PANIC,
    }
}

/// Structured thread-local error. Probes/copies do not clear or replace it.
/// # Safety
/// Buffer/required pointers follow the contract of `uareu_last_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_last_error_json(
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    match catch_unwind(AssertUnwindSafe(|| {
        let report = LAST_ERROR.with(|last| last.borrow().clone());
        let bytes = serde_json::to_vec(&report)
            .map_err(|cause| error(INTERNAL_ERROR, cause.to_string()))?;
        unsafe { copy_buffer(&bytes, true, buffer, capacity, required) }
    })) {
        Ok(Ok(())) => OK,
        Ok(Err(error)) => error.0,
        Err(_) => PANIC,
    }
}

/// Capabilities of the SDK's supported USB profile, without opening the reader.
/// # Safety
/// Buffer/required pointers follow the contract of `uareu_last_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_list_get_capabilities_json(
    handle: u64,
    index: u32,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let devices = list(handle)?;
        let info = devices
            .get(index as usize)
            .ok_or_else(|| error(INVALID_ARGUMENT, "reader index out of range"))?;
        let capabilities = ReaderCapabilities::for_usb_id(info.vendor_id(), info.product_id())
            .ok_or_else(|| error(DEVICE_ERROR, "unsupported USB profile"))?;
        let bytes = serde_json::to_vec(&capabilities)
            .map_err(|cause| error(INTERNAL_ERROR, cause.to_string()))?;
        unsafe { copy_buffer(&bytes, true, buffer, capacity, required) }
    })
}

/// Cached SDK status, never a live USB/hardware status query.
/// # Safety
/// Buffer/required pointers follow the contract of `uareu_last_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_get_status_json(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let context = reader(handle)?;
        let state = context
            .state
            .lock()
            .map_err(|_| error(INTERNAL_ERROR, "reader state is poisoned"))?;
        if state.closed {
            return Err(error(INVALID_HANDLE, "reader is closed"));
        }
        let mut value = serde_json::to_value(state.monitor.snapshot())
            .map_err(|cause| error(INTERNAL_ERROR, cause.to_string()))?;
        value["busy"] = serde_json::json!(state.active.is_some());
        value["persistent_session"] = serde_json::json!(state.persistent);
        value["cancellation_requested"] = serde_json::json!(
            state
                .active
                .as_ref()
                .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
        );
        let bytes =
            serde_json::to_vec(&value).map_err(|cause| error(INTERNAL_ERROR, cause.to_string()))?;
        drop(state);
        unsafe { copy_buffer(&bytes, true, buffer, capacity, required) }
    })
}

/// Capabilities for an open reader handle, without USB I/O.
/// # Safety
/// Buffer/required pointers follow the contract of `uareu_last_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_get_capabilities_json(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let context = reader(handle)?;
        let state = context
            .state
            .lock()
            .map_err(|_| error(INTERNAL_ERROR, "reader state is poisoned"))?;
        if state.closed {
            return Err(error(INVALID_HANDLE, "reader is closed"));
        }
        let capabilities =
            ReaderCapabilities::for_usb_id(context.info.vendor_id(), context.info.product_id())
                .ok_or_else(|| error(DEVICE_ERROR, "unsupported USB profile"))?;
        let bytes = serde_json::to_vec(&capabilities)
            .map_err(|cause| error(INTERNAL_ERROR, cause.to_string()))?;
        drop(state);
        unsafe { copy_buffer(&bytes, true, buffer, capacity, required) }
    })
}

/// Snapshot connected supported readers without claiming their interfaces.
/// # Safety
/// `output` must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_list_create(output: *mut u64) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let devices = usb::readers().map_err(|cause| mapped(DEVICE_ERROR, cause))?;
        let handle = insert(Object::List(Arc::new(devices)))?;
        unsafe { write(output, handle) }
    })
}

/// # Safety
/// `output` must point to writable u32 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_list_count(handle: u64, output: *mut u32) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let count = u32::try_from(list(handle)?.len())
            .map_err(|_| error(INTERNAL_ERROR, "too many readers"))?;
        unsafe { write(output, count) }
    })
}

/// UTF-8 JSON for one device in the immutable discovery snapshot.
/// # Safety
/// Buffer/required pointers follow the contract of `uareu_last_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_list_get_json(
    handle: u64,
    index: u32,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let devices = list(handle)?;
        let device = devices
            .get(index as usize)
            .ok_or_else(|| error(INVALID_ARGUMENT, "reader index out of range"))?;
        let json = serde_json::json!({ "schema_version": 1, "index": index,
            "vendor_id": device.vendor_id(), "product_id": device.product_id(), "device_version": device.device_version(),
            "product": device.product_string(), "manufacturer": device.manufacturer_string(), "serial_number": device.serial_number() });
        let bytes =
            serde_json::to_vec(&json).map_err(|cause| error(INTERNAL_ERROR, cause.to_string()))?;
        unsafe { copy_buffer(&bytes, true, buffer, capacity, required) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn uareu_reader_list_free(handle: u64) -> u32 {
    boundary(|| {
        release(handle, |object| matches!(object, Object::List(_)))?;
        Ok(())
    })
}

/// Open/claim a reader selected from the snapshot. No fingerprint is acquired.
/// # Safety
/// `output` must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_open(handle: u64, index: u32, output: *mut u64) -> u32 {
    unsafe { uareu_reader_open_ex(handle, index, 0, output) }
}
/// Open flags: bit 0 retains the interface between captures. Other bits fail.
/// # Safety
/// `output` must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_open_ex(
    handle: u64,
    index: u32,
    flags: u32,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        if flags & !1 != 0 {
            return Err(error(INVALID_ARGUMENT, "unknown reader flags"));
        }
        let info = list(handle)?
            .get(index as usize)
            .cloned()
            .ok_or_else(|| error(INVALID_ARGUMENT, "reader index out of range"))?;
        let idle_reader = Reader::open(&info).map_err(|cause| mapped(DEVICE_ERROR, cause))?;
        let monitor = idle_reader.status_monitor();
        let context = ReaderContext {
            info,
            state: Arc::new(Mutex::new(ReaderState {
                closed: false,
                idle_reader: Some(idle_reader),
                active: None,
                monitor,
                persistent: flags & 1 != 0,
            })),
        };
        let id = insert(Object::Reader(Arc::new(context)))?;
        unsafe { write(output, id) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn uareu_reader_close(handle: u64) -> u32 {
    boundary(|| {
        let Object::Reader(context) =
            release(handle, |object| matches!(object, Object::Reader(_)))?
        else {
            unreachable!()
        };
        let mut state = context
            .state
            .lock()
            .map_err(|_| error(INTERNAL_ERROR, "reader state is poisoned"))?;
        state.close();
        Ok(())
    })
}

/// Cooperative cancellation is a no-op while idle; it never poisons the next capture.
#[unsafe(no_mangle)]
pub extern "C" fn uareu_cancel(handle: u64) -> u32 {
    boundary(|| {
        let context = reader(handle)?;
        let state = context
            .state
            .lock()
            .map_err(|_| error(INTERNAL_ERROR, "reader state is poisoned"))?;
        state.cancel()
    })
}

/// One blocking capture. Another thread may call cancel or close on this handle.
/// # Safety
/// `output` must point to writable u64 storage for the entire call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_capture(
    handle: u64,
    timeout_ms: u32,
    rotate180: u32,
    output: *mut u64,
) -> u32 {
    unsafe {
        uareu_capture_with_callback(
            handle,
            timeout_ms,
            rotate180,
            None,
            std::ptr::null_mut(),
            output,
        )
    }
}

/// Optional phase notification, synchronous on the capture thread. Foreign
/// callbacks must not throw/unwind, and their context must outlive this call.
pub type CaptureCallback = unsafe extern "C" fn(u32, *mut std::ffi::c_void);
/// The callback owns each image handle. Return 0 to continue, 1 to stop.
pub type ImageCallback = unsafe extern "C" fn(u64, *mut std::ffi::c_void) -> u32;

/// Repeated placements, not video streaming. Callback owns each image handle.
/// # Safety
/// Callbacks/context must remain valid and must not unwind for the entire call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_capture_continuous(
    handle: u64,
    timeout_ms: u32,
    rotate180: u32,
    callback: Option<ImageCallback>,
    phase: Option<CaptureCallback>,
    callback_context: *mut std::ffi::c_void,
) -> u32 {
    boundary(|| {
        if !(1..=3_600_000).contains(&timeout_ms) || rotate180 > 1 || callback.is_none() {
            return Err(error(
                INVALID_ARGUMENT,
                "invalid continuous capture options/callback",
            ));
        }
        let callback = callback.unwrap();
        let context = reader(handle)?;
        let (guard, idle) = CaptureGuard::begin(context.state.clone())?;
        let mut device = open_operation_reader(&context, idle, false).map_err(capture_error)?;
        let mut callback_error = None;
        let result = device.capture_continuous_detailed(
            CaptureOptions {
                timeout: Duration::from_millis(timeout_ms.into()),
                rotate180: rotate180 == 1,
            },
            &guard.cancel,
            |event| {
                if let Some(phase) = phase {
                    unsafe { phase(phase_number(event), callback_context) }
                }
            },
            |image| {
                let image = match register_image(image) {
                    Ok(handle) => handle,
                    Err(error) => {
                        callback_error = Some(error);
                        return Err(anyhow::anyhow!("cannot register capture image"));
                    }
                };
                match unsafe { callback(image, callback_context) } {
                    0 => Ok(std::ops::ControlFlow::Continue(())),
                    1 => Ok(std::ops::ControlFlow::Break(())),
                    _ => {
                        callback_error =
                            Some(error(INVALID_ARGUMENT, "image callback must return 0 or 1"));
                        Err(anyhow::anyhow!("invalid callback action"))
                    }
                }
            },
        );
        guard.retain_after(device, &result, true);
        if let Some(error) = callback_error {
            return Err(error);
        }
        match result.map_err(capture_error)? {
            crate::protocol::ContinuousStop::Callback => Ok(()),
            crate::protocol::ContinuousStop::Cancelled { stage } => Err(capture_error(
                CaptureError::new(
                    CaptureErrorKind::Cancelled,
                    stage,
                    "continuous capture cancelled",
                )
                .into(),
            )),
        }
    })
}
fn phase_number(event: CaptureEvent) -> u32 {
    match event {
        CaptureEvent::Initializing => 1,
        CaptureEvent::WaitingForFinger => 2,
        CaptureEvent::WaitingForFingerRemoval => 3,
        CaptureEvent::Capturing => 4,
    }
}

/// Start an asynchronous capture, reserving the reader before returning.
/// # Safety
/// `output` must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_capture_start(
    handle: u64,
    timeout_ms: u32,
    rotate180: u32,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        if !(1..=3_600_000).contains(&timeout_ms) || rotate180 > 1 {
            return Err(error(INVALID_ARGUMENT, "invalid capture options"));
        }
        let context = reader(handle)?;
        let (guard, idle) = CaptureGuard::begin(context.state.clone())?;
        let cancel = guard.cancel.clone();
        let job = CaptureTask::spawn(cancel, move |_| {
            capture_once(
                context,
                guard,
                idle,
                CaptureOptions {
                    timeout: Duration::from_millis(timeout_ms.into()),
                    rotate180: rotate180 == 1,
                },
                |_| {},
            )
        })
        .map_err(|cause| mapped(INTERNAL_ERROR, cause))?;
        let id = insert(Object::Task(Arc::new(job)))?;
        unsafe { write(output, id) }
    })
}
/// Wait/poll. completed=0 means pending; completed=1 consumes the result once.
/// Errors are copied to this calling thread's last-error report.
/// # Safety
/// Both output pointers must be writable and nonoverlapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_task_wait(
    handle: u64,
    timeout_ms: u32,
    output: *mut u64,
    completed: *mut u32,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
            write(completed, 0)?;
        }
        if timeout_ms > 3_600_000 {
            return Err(error(INVALID_ARGUMENT, "wait timeout exceeds one hour"));
        }
        let job = task(handle)?;
        if job.status().consumed {
            return Err(error(
                INVALID_ARGUMENT,
                "capture completion already consumed",
            ));
        }
        match job.wait(Duration::from_millis(timeout_ms.into())) {
            Ok(None) => Ok(()),
            Ok(Some(image)) => {
                unsafe {
                    write(completed, 1)?;
                }
                let id = register_image(image)?;
                unsafe { write(output, id) }
            }
            Err(cause) => {
                if cause.is::<crate::task::TaskError>()
                    && cause.downcast_ref::<crate::task::TaskError>()
                        != Some(&crate::task::TaskError::WorkerPanicked)
                {
                    return Err(error(INVALID_ARGUMENT, cause.to_string()));
                }
                unsafe {
                    write(completed, 1)?;
                }
                Err(capture_error(cause))
            }
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn uareu_task_cancel(handle: u64) -> u32 {
    boundary(|| {
        task(handle)?.cancel();
        Ok(())
    })
}
/// # Safety
/// Buffer contract is identical to uareu_last_error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_task_get_status_json(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let bytes = serde_json::to_vec(&task(handle)?.status())
            .map_err(|e| error(INTERNAL_ERROR, e.to_string()))?;
        unsafe { copy_buffer(&bytes, true, buffer, capacity, required) }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn uareu_task_free(handle: u64) -> u32 {
    boundary(|| {
        let Object::Task(job) = release(handle, |object| matches!(object, Object::Task(_)))? else {
            unreachable!()
        };
        job.close().map_err(|e| mapped(INTERNAL_ERROR, e))
    })
}

/// JSON request/response helpers use immutable data handles, separate from images.
/// # Safety
/// Buffer contract is identical to uareu_last_error (including the terminator).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_data_get_json(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let Object::Data(data) = lookup(handle)? else {
            return Err(error(INVALID_HANDLE, "expected data handle"));
        };
        unsafe { copy_buffer(&data, true, buffer, capacity, required) }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn uareu_data_free(handle: u64) -> u32 {
    boundary(|| {
        release(handle, |object| matches!(object, Object::Data(_)))?;
        Ok(())
    })
}
/// Import an immutable JSON value (template/enrollment/encoded-data container).
/// # Safety
/// input must hold length readable bytes and output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_data_from_json(
    input: *const u8,
    length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let value: serde_json::Value = unsafe { json_input(input, length)? };
        let id = data(&value)?;
        unsafe { write(output, id) }
    })
}
unsafe fn json_input<T: serde::de::DeserializeOwned>(input: *const u8, length: usize) -> Result<T> {
    if input.is_null() || length == 0 || length > 16 * 1024 * 1024 {
        return Err(error(INVALID_ARGUMENT, "invalid JSON input size/pointer"));
    }
    serde_json::from_slice(unsafe { std::slice::from_raw_parts(input, length) })
        .map_err(|e| error(INVALID_ARGUMENT, e.to_string()))
}
fn data(value: &impl serde::Serialize) -> Result<u64> {
    let bytes = serde_json::to_vec(value).map_err(|e| error(INTERNAL_ERROR, e.to_string()))?;
    insert(Object::Data(Arc::new(bytes)))
}
/// Capture with the same phase notifications used by the CLI.
/// # Safety
/// `output` follows `uareu_capture`; callback/context must remain valid until
/// return. Callback must not throw, unwind, or unload the library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_capture_with_callback(
    handle: u64,
    timeout_ms: u32,
    rotate180: u32,
    callback: Option<CaptureCallback>,
    callback_context: *mut std::ffi::c_void,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        if !(1..=3_600_000).contains(&timeout_ms) || rotate180 > 1 {
            return Err(error(
                INVALID_ARGUMENT,
                "timeout_ms must be 1..3600000 and rotate180 must be 0 or 1",
            ));
        }
        let context = reader(handle)?;
        let (guard, idle_reader) = CaptureGuard::begin(Arc::clone(&context.state))?;
        let result = capture_once(
            context,
            guard,
            idle_reader,
            CaptureOptions {
                timeout: Duration::from_millis(u64::from(timeout_ms)),
                rotate180: rotate180 == 1,
            },
            |event| {
                if let Some(callback) = callback {
                    let phase = match event {
                        CaptureEvent::Initializing => 1,
                        CaptureEvent::WaitingForFinger => 2,
                        CaptureEvent::WaitingForFingerRemoval => 3,
                        CaptureEvent::Capturing => 4,
                    };
                    unsafe { callback(phase, callback_context) };
                }
            },
        );
        let captured = result.map_err(capture_error)?;
        let id = register_image(captured)?;
        unsafe { write(output, id) }
    })
}

/// # Safety
/// `output` must hold `output_size` writable bytes, at least sizeof(ImageInfo), and be aligned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_get_info(
    handle: u64,
    output: *mut ImageInfo,
    output_size: usize,
) -> u32 {
    boundary(|| {
        if output_size < std::mem::size_of::<ImageInfo>() {
            return Err(error(INVALID_ARGUMENT, "image-info structure is too small"));
        }
        valid_output(output)?;
        let image = image(handle)?;
        let pixels = &image.captured.image;
        let width = u32::try_from(pixels.width)
            .map_err(|_| error(IMAGE_ERROR, "image width exceeds C ABI"))?;
        let height = u32::try_from(pixels.height)
            .map_err(|_| error(IMAGE_ERROR, "image height exceeds C ABI"))?;
        unsafe {
            write(
                output,
                ImageInfo {
                    width,
                    height,
                    stride_bytes: width,
                    bits_per_pixel: 8,
                },
            )
        }
    })
}

/// Copy row-major 8-bit grayscale pixels. The required size excludes any terminator.
/// # Safety
/// Buffer/required pointers follow the contract of `uareu_last_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_get_pixels(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let image = image(handle)?;
        unsafe {
            copy_buffer(
                &image.captured.image.pixels,
                false,
                buffer,
                capacity,
                required,
            )
        }
    })
}

/// UTF-8 JSON metadata and image diagnostics. No NFIQ engine is run.
/// # Safety
/// Buffer/required pointers follow the contract of `uareu_last_error`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_get_report_json(
    handle: u64,
    buffer: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> u32 {
    boundary(|| {
        unsafe {
            write(required, 0)?;
        }
        let image = image(handle)?;
        unsafe { copy_buffer(&image.report, true, buffer, capacity, required) }
    })
}

/// Save a new PNG with metadata; existing paths are never overwritten.
/// # Safety
/// `path` must point to `path_length` readable UTF-8 bytes; no NUL is included.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_save_png(
    handle: u64,
    path: *const u8,
    path_length: usize,
) -> u32 {
    boundary(|| {
        let path = unsafe { utf8_path(path, path_length)? };
        image(handle)?
            .captured
            .save_png(path)
            .map_err(|cause| mapped(IMAGE_ERROR, cause))
    })
}

/// Inspect an existing PNG without USB access; embedded metadata remains a declaration.
/// # Safety
/// `path` must point to `path_length` readable UTF-8 bytes, and `output` to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_image_load_png(
    path: *const u8,
    path_length: usize,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let path = unsafe { utf8_path(path, path_length)? };
        let captured = CapturedImage::load_png(path).map_err(|cause| mapped(IMAGE_ERROR, cause))?;
        let id = register_image(captured)?;
        unsafe { write(output, id) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn uareu_image_free(handle: u64) -> u32 {
    boundary(|| {
        release(handle, |object| matches!(object, Object::Image(_)))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::null_mut;

    fn last_error() -> String {
        let mut required = 0;
        assert_eq!(
            unsafe { uareu_last_error(null_mut(), 0, &mut required) },
            BUFFER_TOO_SMALL
        );
        let mut bytes = vec![0; required];
        assert_eq!(
            unsafe { uareu_last_error(bytes.as_mut_ptr(), bytes.len(), &mut required) },
            OK
        );
        String::from_utf8(bytes[..bytes.len() - 1].to_vec()).unwrap()
    }

    #[test]
    fn structured_error_getters_preserve_error_and_legacy_codes() {
        for (kind, legacy) in [
            (CaptureErrorKind::Timeout, DEVICE_ERROR),
            (CaptureErrorKind::Disconnected, DEVICE_ERROR),
            (CaptureErrorKind::Cancelled, CANCELLED),
        ] {
            assert_eq!(
                boundary(|| Err(mapped(
                    legacy,
                    CaptureError::new(kind, CaptureStage::FingerWait, "injected cause").into()
                ))),
                legacy
            );
            let mut required = 0;
            assert_eq!(
                unsafe { uareu_last_error_json(null_mut(), 0, &mut required) },
                BUFFER_TOO_SMALL
            );
            let mut tiny = [99u8; 1];
            assert_eq!(
                unsafe { uareu_last_error_json(tiny.as_mut_ptr(), 1, &mut required) },
                BUFFER_TOO_SMALL
            );
            assert_eq!(tiny, [99]);
            let mut bytes = vec![0; required];
            assert_eq!(
                unsafe { uareu_last_error_json(bytes.as_mut_ptr(), bytes.len(), &mut required) },
                OK
            );
            let report: serde_json::Value =
                serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap();
            assert_eq!(report["status"], legacy);
            assert_eq!(report["kind"], serde_json::to_value(kind).unwrap());
            assert_eq!(report["stage"], "finger_wait");
            assert!(last_error().contains("injected cause"));
            std::thread::spawn(|| {
                assert_eq!(
                    boundary(|| Err(error(INVALID_ARGUMENT, "other thread"))),
                    INVALID_ARGUMENT
                );
                assert_eq!(
                    LAST_ERROR.with(|last| last.borrow().kind.clone()),
                    "invalid_argument"
                );
            })
            .join()
            .unwrap();
            assert!(last_error().contains("injected cause"));
        }
        assert_eq!(boundary(|| Ok(())), OK);
        assert_eq!(LAST_ERROR.with(|last| last.borrow().kind.clone()), "none");
        let mut required = 123;
        assert_eq!(
            unsafe { uareu_reader_get_status_json(0, null_mut(), 0, &mut required) },
            INVALID_HANDLE
        );
        assert_eq!(required, 0);
        assert_eq!(
            unsafe { uareu_reader_get_capabilities_json(0, null_mut(), 0, &mut required) },
            INVALID_HANDLE
        );
        assert_eq!(
            unsafe { uareu_reader_list_get_capabilities_json(0, 0, null_mut(), 0, &mut required) },
            INVALID_HANDLE
        );
    }

    #[test]
    fn image_abi_preserves_pixels_metadata_and_ownership() {
        let captured = crate::frame::decode_frame_with_metadata(
            include_bytes!("../fixtures/synthetic-plain.wire"),
            true,
            |_| unreachable!(),
        )
        .unwrap();
        let expected_pixels = captured.image.pixels.clone();
        let expected_metadata = serde_json::to_value(&captured.metadata).unwrap();
        let id = register_image(captured).unwrap();
        let mut info = ImageInfo::default();
        assert_eq!(unsafe { uareu_image_get_info(id, &mut info, 16) }, OK);
        assert_eq!(
            (
                info.width,
                info.height,
                info.stride_bytes,
                info.bits_per_pixel
            ),
            (384, 290, 384, 8)
        );
        assert_eq!(
            unsafe { uareu_image_get_info(id, &mut info, 15) },
            INVALID_ARGUMENT
        );

        let mut required = 0;
        assert_eq!(
            unsafe { uareu_image_get_pixels(id, null_mut(), 0, &mut required) },
            BUFFER_TOO_SMALL
        );
        assert_eq!(required, expected_pixels.len());
        let mut tiny = [123; 2];
        assert_eq!(
            unsafe { uareu_image_get_pixels(id, tiny.as_mut_ptr(), tiny.len(), &mut required) },
            BUFFER_TOO_SMALL
        );
        assert_eq!(tiny, [123; 2]);
        let mut pixels = vec![0; required];
        assert_eq!(
            unsafe { uareu_image_get_pixels(id, pixels.as_mut_ptr(), pixels.len(), &mut required) },
            OK
        );
        assert_eq!(pixels, expected_pixels);
        assert_eq!(
            unsafe { uareu_image_get_report_json(id, null_mut(), 0, &mut required) },
            BUFFER_TOO_SMALL
        );
        let mut report = vec![0; required];
        assert_eq!(
            unsafe {
                uareu_image_get_report_json(id, report.as_mut_ptr(), report.len(), &mut required)
            },
            OK
        );
        assert_eq!(report.last(), Some(&0));
        let report: serde_json::Value =
            serde_json::from_slice(&report[..report.len() - 1]).unwrap();
        assert_eq!(report["metadata"], expected_metadata);
        assert_eq!(report["quality"]["status"], "not_evaluated");
        let mut count = 123;
        assert_eq!(
            unsafe { uareu_reader_list_count(id, &mut count) },
            INVALID_HANDLE
        );
        assert_eq!(count, 0);
        assert_eq!(uareu_reader_list_free(id), INVALID_HANDLE);
        let retained = image(id).unwrap();
        assert_eq!(uareu_image_free(id), OK);
        assert_eq!(uareu_image_free(id), INVALID_HANDLE);
        assert_eq!(retained.captured.image.pixels, expected_pixels);
    }

    #[test]
    fn invalid_arguments_fail_before_usb_and_errors_are_thread_local() {
        assert_eq!(
            unsafe { uareu_reader_list_create(null_mut()) },
            INVALID_ARGUMENT
        );
        let mut storage = [0u64; 2];
        let misaligned = unsafe { storage.as_mut_ptr().cast::<u8>().add(1).cast::<u64>() };
        assert_eq!(
            unsafe { uareu_reader_list_create(misaligned) },
            INVALID_ARGUMENT
        );
        let mut output = 123;
        assert_eq!(
            unsafe { uareu_capture(0, 0, 1, &mut output) },
            INVALID_ARGUMENT
        );
        assert_eq!(output, 0);
        assert!(last_error().contains("timeout_ms"));
        std::thread::spawn(|| {
            assert_eq!(uareu_image_free(0), INVALID_HANDLE);
            assert!(last_error().contains("handle"));
        })
        .join()
        .unwrap();
        assert!(last_error().contains("timeout_ms"));
        let mut required = 0;
        assert_eq!(
            unsafe { uareu_last_error(null_mut(), 99, &mut required) },
            INVALID_ARGUMENT
        );
        assert!(last_error().contains("timeout_ms"));
        assert_eq!(
            unsafe { uareu_image_load_png([255].as_ptr(), 1, &mut output) },
            INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { uareu_image_load_png(b"bad\0path".as_ptr(), 8, &mut output) },
            INVALID_ARGUMENT
        );
    }

    #[test]
    fn capture_state_serializes_operations_and_close_cancels_active_work() {
        let shared = Arc::new(Mutex::new(ReaderState {
            closed: false,
            idle_reader: None,
            active: None,
            monitor: ReaderMonitor::default(),
            persistent: false,
        }));
        shared.lock().unwrap().cancel().unwrap();
        let (first, _) = CaptureGuard::begin(Arc::clone(&shared)).unwrap();
        let monitor = shared.lock().unwrap().monitor.clone();
        assert!(monitor.snapshot().capture_active);
        assert!(!first.cancel.load(Ordering::Relaxed));
        assert!(matches!(
            CaptureGuard::begin(Arc::clone(&shared)),
            Err(Error(BUSY, _, _))
        ));
        let other = Arc::clone(&shared);
        std::thread::spawn(move || other.lock().unwrap().cancel().unwrap())
            .join()
            .unwrap();
        assert!(first.cancel.load(Ordering::Relaxed));
        drop(first);
        assert!(!monitor.snapshot().capture_active);
        let (second, _) = CaptureGuard::begin(Arc::clone(&shared)).unwrap();
        assert!(
            !second.cancel.load(Ordering::Relaxed),
            "a previous cancellation must not affect the next call"
        );
        shared.lock().unwrap().close();
        assert_eq!(
            monitor.snapshot().operation,
            crate::reader::ReaderOperation::Closed
        );
        assert!(second.cancel.load(Ordering::Relaxed));
        drop(second);
        assert!(matches!(
            CaptureGuard::begin(Arc::clone(&shared)),
            Err(Error(INVALID_HANDLE, _, _))
        ));
    }

    #[test]
    fn cleanup_disconnect_discards_session_without_replacing_primary_error() {
        let primary: anyhow::Error = CaptureError::new(
            CaptureErrorKind::Timeout,
            CaptureStage::Acquisition,
            "deadline",
        )
        .into();
        assert!(!is_disconnected(&primary));
        let combined = crate::error::attach_cleanup(
            primary,
            Err(CaptureError::new(
                CaptureErrorKind::Disconnected,
                CaptureStage::Cleanup,
                "reader unplugged",
            )
            .into()),
        );
        let combined = crate::error::attach_cleanup(
            combined,
            Err(CaptureError::new(
                CaptureErrorKind::Transport,
                CaptureStage::Cleanup,
                "queue drain failed",
            )
            .into()),
        );
        assert!(is_disconnected(&combined));
        let report = CaptureFailure::from_error(&combined);
        assert_eq!(report.kind, CaptureErrorKind::Timeout);
        let first = report.cleanup.unwrap();
        assert_eq!(first.kind, CaptureErrorKind::Disconnected);
        assert_eq!(first.cleanup.unwrap().kind, CaptureErrorKind::Transport);
        assert_eq!(capture_error(combined).0, DEVICE_ERROR);
    }
    #[test]
    fn panics_are_contained_and_capture_lease_is_released() {
        let shared = Arc::new(Mutex::new(ReaderState {
            closed: false,
            idle_reader: None,
            active: None,
            monitor: ReaderMonitor::default(),
            persistent: false,
        }));
        assert_eq!(
            boundary(|| {
                let (_guard, _) = CaptureGuard::begin(Arc::clone(&shared))?;
                panic!("injected FFI failure");
            }),
            PANIC
        );
        assert!(last_error().contains("panic contained"));
        assert!(shared.lock().unwrap().active.is_none());
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .monitor
                .snapshot()
                .last_outcome
                .unwrap()
                .failure
                .unwrap()
                .kind,
            CaptureErrorKind::Panic
        );
        assert!(CaptureGuard::begin(shared.clone()).is_ok());
        assert_eq!(
            boundary(|| {
                let (_guard, _) = CaptureGuard::begin_mode(shared.clone(), false)?;
                panic!("injected probe failure");
            }),
            PANIC
        );
        let state = shared.lock().unwrap();
        assert!(state.active.is_none());
        let status = state.monitor.snapshot();
        assert_eq!(status.operation, crate::reader::ReaderOperation::Idle);
        assert_eq!(
            status.last_probe_failure.unwrap().kind,
            CaptureErrorKind::Panic
        );
    }
}
