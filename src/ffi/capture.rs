// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

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
/// Completion callback for this SDK's independent C ABI.
pub type CaptureCompletionCallback = unsafe extern "C" fn(u32, u64, u64, *mut std::ffi::c_void);

struct CompletionGate {
    enabled: Mutex<Option<bool>>,
    ready: std::sync::Condvar,
}
impl CompletionGate {
    fn release(&self, enabled: bool) {
        *self.enabled.lock().unwrap_or_else(|p| p.into_inner()) = Some(enabled);
        self.ready.notify_all();
    }
    fn wait(&self) -> bool {
        let mut enabled = self.enabled.lock().unwrap_or_else(|p| p.into_inner());
        while enabled.is_none() {
            enabled = self.ready.wait(enabled).unwrap_or_else(|p| p.into_inner());
        }
        enabled.unwrap_or(false)
    }
}
fn deliver_completion(
    callback: CaptureCompletionCallback,
    context: usize,
    result: anyhow::Result<CapturedImage>,
) {
    let result = result.map_err(capture_error).and_then(register_image);
    let (status, image, report) = match result {
        Ok(image) => (OK, image, 0),
        Err(failure) => {
            let status = failure.0;
            let report = data(&ErrorReport::from_failure(failure)).unwrap_or(0);
            (status, 0, report)
        }
    };
    // Caller guarantees executable callback code and a valid context until free
    // finishes joining the worker. No SDK lock is held across this call.
    unsafe {
        callback(status, image, report, context as *mut std::ffi::c_void);
    }
}
pub(super) fn start_callback_operation(
    cancel: Arc<AtomicBool>,
    operation: impl FnOnce(Arc<AtomicBool>) -> anyhow::Result<CapturedImage> + Send + 'static,
    callback: CaptureCompletionCallback,
    context: usize,
    output: *mut u64,
) -> Result<()> {
    let gate = Arc::new(CompletionGate {
        enabled: Mutex::new(None),
        ready: std::sync::Condvar::new(),
    });
    let delivery = gate.clone();
    let job = Arc::new(
        CaptureTask::spawn_with_completion(cancel, operation, move |result| {
            if delivery.wait() {
                deliver_completion(callback, context, result);
            }
        })
        .map_err(|e| mapped(INTERNAL_ERROR, e))?,
    );
    // Keep an Arc while attempting insertion so its failure cannot drop/join a
    // worker still waiting for this gate.
    let id = match insert(Object::Task(job.clone())) {
        Ok(id) => id,
        Err(error) => {
            gate.release(false);
            let _ = job.close();
            return Err(error);
        }
    };
    if let Err(error) = unsafe { write(output, id) } {
        gate.release(false);
        let _ = job.close();
        let _ = release(id, |o| matches!(o, Object::Task(_)));
        return Err(error);
    }
    gate.release(true);
    Ok(())
}
/// Start capture with one worker-thread completion notification. This is the
/// SDK's own C ABI, not a dpfpdd callback or calling-convention alias.
/// # Safety
/// output must be writable; callback/context must remain valid until task free
/// returns. The callback must not unwind or unload the library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uareu_capture_start_with_completion(
    handle: u64,
    timeout_ms: u32,
    rotate180: u32,
    callback: Option<CaptureCompletionCallback>,
    context: *mut std::ffi::c_void,
    output: *mut u64,
) -> u32 {
    boundary(|| {
        unsafe {
            write(output, 0)?;
        }
        let callback =
            callback.ok_or_else(|| error(INVALID_ARGUMENT, "completion callback required"))?;
        if !(1..=3_600_000).contains(&timeout_ms) || rotate180 > 1 {
            return Err(error(INVALID_ARGUMENT, "invalid capture options"));
        }
        let reader = reader(handle)?;
        let (guard, idle) = CaptureGuard::begin(reader.state.clone())?;
        let cancel = guard.cancel.clone();
        start_callback_operation(
            cancel,
            move |_| {
                capture_once(
                    reader,
                    guard,
                    idle,
                    CaptureOptions {
                        timeout: Duration::from_millis(timeout_ms.into()),
                        rotate180: rotate180 == 1,
                    },
                    |_| {},
                )
            },
            callback,
            context as usize,
            output,
        )
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
        let job = task(handle)?;
        job.close().map_err(|e| {
            if e.downcast_ref::<crate::task::TaskError>()
                == Some(&crate::task::TaskError::CloseFromWorker)
            {
                error(BUSY, e.to_string())
            } else {
                mapped(INTERNAL_ERROR, e)
            }
        })?;
        release(handle, |object| matches!(object, Object::Task(_)))?;
        Ok(())
    })
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
