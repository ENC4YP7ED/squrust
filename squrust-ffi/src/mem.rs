//! Helpers for C-allocated strings the caller frees with `sqlite3_free`.

use std::os::raw::c_char;
use std::ptr;

/// Duplicate a Rust string into a `libc::malloc`-allocated, NUL-terminated C
/// string. Returns NULL on allocation failure.
///
/// # Safety
/// The returned pointer must be freed with `sqlite3_free` (i.e. `libc::free`).
pub unsafe fn c_string_dup(s: &str) -> *mut c_char {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let buf = libc::malloc(len + 1) as *mut u8;
    if buf.is_null() {
        return ptr::null_mut();
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);
    *buf.add(len) = 0;
    buf as *mut c_char
}
