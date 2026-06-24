//! Version, change-counting, and assorted no-op compatibility shims.

use std::os::raw::{c_char, c_int};

use crate::constants::*;
use crate::state::{conn, stmt};
use crate::types::{sqlite3, sqlite3_stmt};

#[no_mangle]
pub extern "C" fn sqlite3_libversion() -> *const c_char {
    c"3.45.0".as_ptr()
}

#[no_mangle]
pub extern "C" fn sqlite3_libversion_number() -> c_int {
    3_045_000
}

#[no_mangle]
pub extern "C" fn sqlite3_sourceid() -> *const c_char {
    c"squrust-0.1.0".as_ptr()
}

#[no_mangle]
pub extern "C" fn sqlite3_threadsafe() -> c_int {
    1
}

/// # Safety
/// `db` from `sqlite3_open*` or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_changes(db: *mut sqlite3) -> c_int {
    match conn(db) {
        Some(c) => c.engine.changes() as c_int,
        None => 0,
    }
}

/// # Safety
/// See [`sqlite3_changes`].
#[no_mangle]
pub unsafe extern "C" fn sqlite3_changes64(db: *mut sqlite3) -> i64 {
    match conn(db) {
        Some(c) => c.engine.changes(),
        None => 0,
    }
}

/// # Safety
/// See [`sqlite3_changes`].
#[no_mangle]
pub unsafe extern "C" fn sqlite3_total_changes(db: *mut sqlite3) -> c_int {
    sqlite3_changes(db)
}

/// # Safety
/// `db` from `sqlite3_open*` or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_last_insert_rowid(db: *mut sqlite3) -> i64 {
    match conn(db) {
        Some(c) => c.engine.last_insert_rowid(),
        None => 0,
    }
}

/// # Safety
/// `db` from `sqlite3_open*` or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_interrupt(_db: *mut sqlite3) {
    // Cancellation is not yet wired through; no-op.
}

/// # Safety
/// `db` from `sqlite3_open*` or NULL. Returns 0 while an explicit `BEGIN`
/// transaction is in progress, 1 otherwise.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_get_autocommit(db: *mut sqlite3) -> c_int {
    match conn(db) {
        Some(c) if c.tx.is_some() => 0,
        _ => 1,
    }
}

/// # Safety
/// `db` from `sqlite3_open*` or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_busy_timeout(_db: *mut sqlite3, _ms: c_int) -> c_int {
    SQLITE_OK
}

/// # Safety
/// `db` from `sqlite3_open*` or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_extended_result_codes(_db: *mut sqlite3, _on: c_int) -> c_int {
    SQLITE_OK
}

/// Returns 1 if `sql` appears to be a complete statement (ends with `;`).
///
/// # Safety
/// `sql` must be NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_complete(sql: *const c_char) -> c_int {
    if sql.is_null() {
        return 0;
    }
    let s = std::ffi::CStr::from_ptr(sql).to_string_lossy();
    i32::from(s.trim_end().ends_with(';'))
}

/// # Safety
/// `s` from `sqlite3_prepare_v2` or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_db_handle(s: *mut sqlite3_stmt) -> *mut sqlite3 {
    match stmt(s) {
        Some(st) => st.db as *mut sqlite3,
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn sqlite3_initialize() -> c_int {
    SQLITE_OK
}

#[no_mangle]
pub extern "C" fn sqlite3_shutdown() -> c_int {
    SQLITE_OK
}

#[no_mangle]
pub extern "C" fn sqlite3_enable_shared_cache(_enable: c_int) -> c_int {
    SQLITE_OK
}
