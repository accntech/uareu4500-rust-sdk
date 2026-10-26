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
fn engine_data<T: serde::Serialize>(value: anyhow::Result<T>) -> Result<u64> {
    data(&value.map_err(|e| mapped(IMAGE_ERROR, e))?)
}
fn read_data<T: serde::de::DeserializeOwned>(handle: u64) -> Result<T> {
    let Object::Data(bytes) = lookup(handle)? else {
        return Err(error(INVALID_HANDLE, "expected data handle"));
    };
    serde_json::from_slice(&bytes).map_err(|e| error(INVALID_ARGUMENT, e.to_string()))
}

mod errors;
pub use errors::*;
mod readers;
pub use readers::*;
mod capture;
pub use capture::*;
mod data;
pub use data::*;
mod images;
pub use images::*;
mod interchange;
pub use interchange::*;
mod assessment;
pub use assessment::*;
mod matching;
pub use matching::*;

#[cfg(test)]
use capture::start_callback_operation;
#[cfg(test)]
use readers::wait_reader_status;
#[cfg(test)]
mod tests;
