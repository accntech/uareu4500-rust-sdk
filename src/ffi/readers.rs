// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

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

/// Wait for a cached status sequence change; no USB polling is performed.
/// # Safety
/// completed and output must point to writable u32/u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_reader_wait_status_json(
    handle: u64,
    after_sequence: u64,
    timeout_ms: u32,
    completed: *mut u32,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
            write(completed, 0)?;
        }
        if timeout_ms > 300_000 {
            return Err(error(INVALID_ARGUMENT, "status wait deadline out of range"));
        }
        let context = reader(handle)?;
        if let Some(status) = wait_reader_status(&context.state, after_sequence, timeout_ms)? {
            let id = data(&status)?;
            unsafe {
                write(output, id)?;
                write(completed, 1)?;
            }
        }
        Ok(())
    })
}

pub(super) fn wait_reader_status(
    state: &Mutex<ReaderState>,
    after_sequence: u64,
    timeout_ms: u32,
) -> Result<Option<crate::reader::ReaderStatus>> {
    let monitor = {
        let state = state
            .lock()
            .map_err(|_| error(INTERNAL_ERROR, "reader state is poisoned"))?;
        if state.closed {
            return Err(error(INVALID_HANDLE, "reader is closed"));
        }
        state.monitor.clone()
    };
    Ok(monitor.wait_for_change(after_sequence, Duration::from_millis(u64::from(timeout_ms))))
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
