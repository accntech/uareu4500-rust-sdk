// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

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
