//! Versioned UTF-8 JSON C ABI. Only this crate owns unsafe pointer transport.
use octri_converter::{ConversionError, MAX_ENVELOPE_BYTES};
use std::{panic::catch_unwind, ptr, slice, str};
const MAX_INPUT_BYTES: usize = MAX_ENVELOPE_BYTES;
#[derive(Debug)]
pub struct Buffer(Vec<u8>);
fn error(code: &str, message: impl Into<String>) -> String {
    octri_converter::wire::encode::<()>(Err(ConversionError::new(code, message)))
}
pub fn dispatch(input: &[u8]) -> String {
    if input.len() > MAX_INPUT_BYTES {
        return error("LIMIT_REQUEST_BYTES", "Converter envelope exceeds 128 MiB");
    }
    match str::from_utf8(input) {
        Ok(input) => octri_converter::wire::dispatch(input),
        Err(_) => error("INVALID_ENCODING", "Converter request must be UTF-8"),
    }
}
fn owned(bytes: Vec<u8>) -> *mut Buffer {
    Box::into_raw(Box::new(Buffer(bytes)))
}

#[unsafe(no_mangle)]
pub extern "C" fn octri_converter_abi_version() -> u32 {
    1
}

/// Allocate an input buffer. Zero and oversized allocations return null.
#[unsafe(no_mangle)]
pub extern "C" fn octri_converter_buffer_new(length: usize) -> *mut Buffer {
    if length == 0 || length > MAX_INPUT_BYTES {
        return ptr::null_mut();
    }
    owned(vec![0; length])
}

/// Execute one isolated call. Null plus zero is accepted as an empty request.
/// Panics become an error envelope; allocation failure remains process-fatal.
///
/// # Safety
/// Non-null `data` must reference `length` readable bytes for this call. Hosts
/// must not modify those bytes concurrently. Returned buffer must be freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn octri_converter_call(data: *const u8, length: usize) -> *mut Buffer {
    let result = if length > MAX_INPUT_BYTES {
        error("LIMIT_REQUEST_BYTES", "Binding request exceeds 128 MiB")
    } else if data.is_null() && length != 0 {
        error(
            "INVALID_REQUEST",
            "Nonempty request has a null data pointer",
        )
    } else {
        // SAFETY: The caller guarantees validity for nonempty input. A null
        // pointer is never passed to from_raw_parts, even for an empty slice.
        let input = if length == 0 {
            &[]
        } else {
            // SAFETY: Nonempty input is readable for length bytes by contract.
            unsafe { slice::from_raw_parts(data, length) }
        };
        catch_unwind(|| dispatch(input))
            .unwrap_or_else(|_| error("INTERNAL_ERROR", "Native converter panicked"))
    };
    owned(result.into_bytes())
}

/// # Safety
/// `buffer` is null or a live allocation returned by this library. Only freshly
/// allocated input buffers may be written; response bytes are read-only by contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn octri_converter_buffer_data(buffer: *mut Buffer) -> *mut u8 {
    if buffer.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: A live allocation is required by the function contract.
    unsafe { (*buffer).0.as_ptr().cast_mut() }
}

/// # Safety
/// `buffer` is null or a live allocation returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn octri_converter_buffer_len(buffer: *const Buffer) -> usize {
    if buffer.is_null() {
        return 0;
    }
    // SAFETY: A live allocation is required by the function contract.
    unsafe { (*buffer).0.len() }
}

/// Release through the same loaded library that allocated the buffer. Null is safe.
///
/// # Safety
/// A non-null buffer must be a live allocation from this library, freed exactly
/// once. No accessors, reads or writes may overlap its release.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn octri_converter_buffer_free(buffer: *mut Buffer) {
    if !buffer.is_null() {
        // SAFETY: The caller transfers its unique allocation back exactly once.
        drop(unsafe { Box::from_raw(buffer) });
    }
}
