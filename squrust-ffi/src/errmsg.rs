//! Error reporting and the C allocator hook.

use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

use crate::constants::*;
use crate::state::conn;
use crate::types::sqlite3;

/// # Safety
/// `db` from `sqlite3_open*` or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_errcode(db: *mut sqlite3) -> c_int {
    match conn(db) {
        Some(c) => c.last_errcode,
        None => SQLITE_OK,
    }
}

/// # Safety
/// See [`sqlite3_errcode`].
#[no_mangle]
pub unsafe extern "C" fn sqlite3_extended_errcode(db: *mut sqlite3) -> c_int {
    sqlite3_errcode(db)
}

/// # Safety
/// `db` from `sqlite3_open*` or NULL. The returned pointer is owned by the
/// connection and valid until the next call that changes the error.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char {
    match conn(db) {
        Some(c) => match &c.last_error {
            Some(cs) => cs.as_ptr(),
            None => c"not an error".as_ptr(),
        },
        None => c"out of memory".as_ptr(),
    }
}

/// # Safety
/// See [`sqlite3_errmsg`]. UTF-16 is not supported; returns NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_errmsg16(_db: *mut sqlite3) -> *const c_void {
    ptr::null()
}

/// Human-readable string for a result code.
#[no_mangle]
pub extern "C" fn sqlite3_errstr(code: c_int) -> *const c_char {
    let s = match code {
        SQLITE_OK => c"not an error",
        SQLITE_ERROR => c"SQL logic error",
        SQLITE_BUSY => c"database is locked",
        SQLITE_CANTOPEN => c"unable to open database file",
        SQLITE_MISUSE => c"bad parameter or other API misuse",
        SQLITE_RANGE => c"column index out of range",
        SQLITE_CORRUPT => c"database disk image is malformed",
        _ => c"unknown error",
    };
    s.as_ptr()
}

/// Free memory allocated by Squrust for the caller (e.g. `sqlite3_exec` error
/// messages).
///
/// # Safety
/// `ptr` must have come from a Squrust API that documents `sqlite3_free`, or be NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_free(ptr: *mut c_void) {
    if !ptr.is_null() {
        libc::free(ptr);
    }
}

/// Allocate `n` bytes with the C allocator.
///
/// # Safety
/// Returned pointer must be released with [`sqlite3_free`].
#[no_mangle]
pub unsafe extern "C" fn sqlite3_malloc(n: c_int) -> *mut c_void {
    if n <= 0 {
        ptr::null_mut()
    } else {
        libc::malloc(n as usize)
    }
}
