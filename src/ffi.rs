// SPDX-License-Identifier: GPL-3.0-or-later
//! Version 1 C ABI. See `include/uareu.h` for ownership and concurrency rules.
//! Existing Rust capture APIs do not use this registry or change their behavior.
use crate::{
    metadata::CapturedImage,
    protocol::{CaptureEvent, CaptureOptions},
    quality,
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
#[derive(Debug)]
struct Error(u32, String);
fn error(code: u32, message: impl Into<String>) -> Error {
    Error(code, message.into())
}
fn mapped(code: u32, cause: anyhow::Error) -> Error {
    error(code, format!("{cause:#}"))
}

thread_local! { static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) }; }

fn boundary(operation: impl FnOnce() -> Result<()>) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(operation));
    let failure = match result {
        Ok(Ok(())) => {
            LAST_ERROR.with(|last| last.borrow_mut().clear());
            return OK;
        }
        Ok(Err(error)) => error,
        Err(_) => error(PANIC, "Rust panic contained at the DLL boundary"),
    };
    LAST_ERROR.with(|last| *last.borrow_mut() = failure.1);
    failure.0
}

struct ReaderState {
    closed: bool,
    idle_reader: Option<Reader>,
    active: Option<Arc<AtomicBool>>,
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
        }
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
        let message = LAST_ERROR.with(|last| last.borrow().clone());
        unsafe { copy_buffer(message.as_bytes(), true, buffer, capacity, required) }
    })) {
        Ok(Ok(())) => OK,
        Ok(Err(error)) => error.0,
        Err(_) => PANIC,
    }
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
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let info = list(handle)?
            .get(index as usize)
            .cloned()
            .ok_or_else(|| error(INVALID_ARGUMENT, "reader index out of range"))?;
        let idle_reader = Reader::open(&info).map_err(|cause| mapped(DEVICE_ERROR, cause))?;
        let context = ReaderContext {
            info,
            state: Arc::new(Mutex::new(ReaderState {
                closed: false,
                idle_reader: Some(idle_reader),
                active: None,
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
        let device = match idle_reader {
            Some(reader) => reader,
            None => Reader::open(&context.info).map_err(|cause| mapped(DEVICE_ERROR, cause))?,
        };
        let result = device.capture_with_metadata(
            CaptureOptions {
                timeout: Duration::from_millis(u64::from(timeout_ms)),
                rotate180: rotate180 == 1,
            },
            &guard.cancel,
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
        let captured = match result {
            Ok(captured) => captured,
            Err(cause) => {
                return Err(mapped(
                    if guard.cancel.load(Ordering::Relaxed) {
                        CANCELLED
                    } else {
                        DEVICE_ERROR
                    },
                    cause,
                ));
            }
        };
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
        }));
        shared.lock().unwrap().cancel().unwrap();
        let (first, _) = CaptureGuard::begin(Arc::clone(&shared)).unwrap();
        assert!(!first.cancel.load(Ordering::Relaxed));
        assert!(matches!(
            CaptureGuard::begin(Arc::clone(&shared)),
            Err(Error(BUSY, _))
        ));
        let other = Arc::clone(&shared);
        std::thread::spawn(move || other.lock().unwrap().cancel().unwrap())
            .join()
            .unwrap();
        assert!(first.cancel.load(Ordering::Relaxed));
        drop(first);
        let (second, _) = CaptureGuard::begin(Arc::clone(&shared)).unwrap();
        assert!(
            !second.cancel.load(Ordering::Relaxed),
            "a previous cancellation must not affect the next call"
        );
        shared.lock().unwrap().close();
        assert!(second.cancel.load(Ordering::Relaxed));
        drop(second);
        assert!(matches!(
            CaptureGuard::begin(Arc::clone(&shared)),
            Err(Error(INVALID_HANDLE, _))
        ));
    }

    #[test]
    fn panics_are_contained_and_capture_lease_is_released() {
        let shared = Arc::new(Mutex::new(ReaderState {
            closed: false,
            idle_reader: None,
            active: None,
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
        assert!(CaptureGuard::begin(shared).is_ok());
    }
}
