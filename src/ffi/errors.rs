// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

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
