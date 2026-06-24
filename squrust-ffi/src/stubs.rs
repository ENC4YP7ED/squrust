//! Compatibility stubs for the long tail of the SQLite C API.
//!
//! Under `LD_PRELOAD`, *every* `sqlite3_*` symbol a host program references must
//! be defined here — otherwise the dynamic linker resolves the missing ones to
//! the real `libsqlite3`, which then operates on a Squrust handle and crashes.
//! Functionality Squrust does not implement (custom functions, blobs, backup,
//! serialize, extensions) is stubbed to fail safely rather than fall through.
//!
//! Variadic SQLite functions (`sqlite3_db_config`, etc.) are declared here as
//! non-variadic and ignore their extra arguments; on the x86-64 SysV ABI a
//! caller's surplus register/stack arguments are simply left unread.

use std::os::raw::{c_char, c_double, c_int, c_void};

use crate::constants::*;
use crate::types::{sqlite3, sqlite3_stmt};

// ---- memory / utility ----

/// # Safety: returns memory the caller frees with `sqlite3_free`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_malloc64(n: u64) -> *mut c_void {
    if n == 0 {
        std::ptr::null_mut()
    } else {
        libc::malloc(n as usize)
    }
}

/// # Safety: `p` was returned by a Squrust allocator or is NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_realloc(p: *mut c_void, n: c_int) -> *mut c_void {
    libc::realloc(p, n.max(0) as usize)
}

#[no_mangle]
pub extern "C" fn sqlite3_sleep(ms: c_int) -> c_int {
    ms.max(0)
}

/// Case-insensitive C-string compare.
/// # Safety: both pointers are NUL-terminated or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_stricmp(a: *const c_char, b: *const c_char) -> c_int {
    if a.is_null() || b.is_null() {
        return (a.is_null() as c_int) - (b.is_null() as c_int);
    }
    let (mut a, mut b) = (a, b);
    loop {
        let ca = (*a as u8).to_ascii_lowercase();
        let cb = (*b as u8).to_ascii_lowercase();
        if ca != cb {
            return ca as c_int - cb as c_int;
        }
        if ca == 0 {
            return 0;
        }
        a = a.add(1);
        b = b.add(1);
    }
}

/// # Safety: see [`sqlite3_stricmp`]; compares at most `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_strnicmp(
    a: *const c_char,
    b: *const c_char,
    n: c_int,
) -> c_int {
    for i in 0..n.max(0) as isize {
        let ca = (*a.offset(i) as u8).to_ascii_lowercase();
        let cb = (*b.offset(i) as u8).to_ascii_lowercase();
        if ca != cb {
            return ca as c_int - cb as c_int;
        }
        if ca == 0 {
            break;
        }
    }
    0
}

/// Generous fixed limits; returns the prior value (we never actually change one).
#[no_mangle]
pub extern "C" fn sqlite3_limit(_db: *mut sqlite3, _id: c_int, _new_val: c_int) -> c_int {
    1_000_000_000
}

// ---- connection-level config / hooks (no-ops returning OK) ----

#[no_mangle]
pub extern "C" fn sqlite3_db_config(_db: *mut sqlite3, _op: c_int) -> c_int {
    SQLITE_OK
}

#[no_mangle]
pub extern "C" fn sqlite3_set_authorizer(
    _db: *mut sqlite3,
    _cb: *mut c_void,
    _arg: *mut c_void,
) -> c_int {
    SQLITE_OK
}

#[no_mangle]
pub extern "C" fn sqlite3_trace_v2(
    _db: *mut sqlite3,
    _mask: c_int,
    _cb: *mut c_void,
    _ctx: *mut c_void,
) -> c_int {
    SQLITE_OK
}

#[no_mangle]
pub extern "C" fn sqlite3_progress_handler(
    _db: *mut sqlite3,
    _n: c_int,
    _cb: *mut c_void,
    _arg: *mut c_void,
) {
}

#[no_mangle]
pub extern "C" fn sqlite3_enable_load_extension(_db: *mut sqlite3, _on: c_int) -> c_int {
    SQLITE_OK
}

#[no_mangle]
pub extern "C" fn sqlite3_load_extension(
    _db: *mut sqlite3,
    _file: *const c_char,
    _entry: *const c_char,
    _errmsg: *mut *mut c_char,
) -> c_int {
    SQLITE_ERROR
}

// ---- user-defined functions / collations (registration accepted, unused) ----

#[no_mangle]
pub extern "C" fn sqlite3_create_function_v2(
    _db: *mut sqlite3,
    _name: *const c_char,
    _nargs: c_int,
    _enc: c_int,
    _app: *mut c_void,
    _func: *mut c_void,
    _step: *mut c_void,
    _final_: *mut c_void,
    _destroy: *mut c_void,
) -> c_int {
    SQLITE_OK
}

#[no_mangle]
pub extern "C" fn sqlite3_create_window_function(
    _db: *mut sqlite3,
    _name: *const c_char,
    _nargs: c_int,
    _enc: c_int,
    _app: *mut c_void,
    _step: *mut c_void,
    _final_: *mut c_void,
    _value: *mut c_void,
    _inverse: *mut c_void,
    _destroy: *mut c_void,
) -> c_int {
    SQLITE_OK
}

#[no_mangle]
pub extern "C" fn sqlite3_create_collation_v2(
    _db: *mut sqlite3,
    _name: *const c_char,
    _enc: c_int,
    _arg: *mut c_void,
    _cmp: *mut c_void,
    _destroy: *mut c_void,
) -> c_int {
    SQLITE_OK
}

// ---- function-context accessors (only reached inside custom callbacks) ----

#[no_mangle]
pub extern "C" fn sqlite3_user_data(_ctx: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn sqlite3_context_db_handle(_ctx: *mut c_void) -> *mut sqlite3 {
    std::ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn sqlite3_aggregate_context(_ctx: *mut c_void, _n: c_int) -> *mut c_void {
    std::ptr::null_mut()
}

// ---- sqlite3_value_* (custom-function argument access) ----

#[no_mangle]
pub extern "C" fn sqlite3_value_type(_v: *mut c_void) -> c_int {
    SQLITE_NULL
}
#[no_mangle]
pub extern "C" fn sqlite3_value_int64(_v: *mut c_void) -> i64 {
    0
}
#[no_mangle]
pub extern "C" fn sqlite3_value_double(_v: *mut c_void) -> c_double {
    0.0
}
#[no_mangle]
pub extern "C" fn sqlite3_value_bytes(_v: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub extern "C" fn sqlite3_value_text(_v: *mut c_void) -> *const c_void {
    std::ptr::null()
}
#[no_mangle]
pub extern "C" fn sqlite3_value_blob(_v: *mut c_void) -> *const c_void {
    std::ptr::null()
}

// ---- sqlite3_result_* (custom-function result; no-ops) ----

#[no_mangle]
pub extern "C" fn sqlite3_result_null(_ctx: *mut c_void) {}
#[no_mangle]
pub extern "C" fn sqlite3_result_int64(_ctx: *mut c_void, _v: i64) {}
#[no_mangle]
pub extern "C" fn sqlite3_result_double(_ctx: *mut c_void, _v: c_double) {}
#[no_mangle]
pub extern "C" fn sqlite3_result_text(
    _ctx: *mut c_void,
    _t: *const c_char,
    _n: c_int,
    _d: *mut c_void,
) {
}
#[no_mangle]
pub extern "C" fn sqlite3_result_blob(
    _ctx: *mut c_void,
    _b: *const c_void,
    _n: c_int,
    _d: *mut c_void,
) {
}
#[no_mangle]
pub extern "C" fn sqlite3_result_error(_ctx: *mut c_void, _msg: *const c_char, _n: c_int) {}
#[no_mangle]
pub extern "C" fn sqlite3_result_error_nomem(_ctx: *mut c_void) {}
#[no_mangle]
pub extern "C" fn sqlite3_result_error_toobig(_ctx: *mut c_void) {}

// ---- statement extras ----

// `sqlite3_stmt_busy` is implemented in `step.rs` (it needs statement state).

/// We don't expand bound parameters into SQL text; callers treat NULL as
/// "unavailable".
#[no_mangle]
pub extern "C" fn sqlite3_expanded_sql(_stmt: *mut sqlite3_stmt) -> *mut c_char {
    std::ptr::null_mut()
}

// ---- blob I/O (unsupported) ----

#[no_mangle]
pub extern "C" fn sqlite3_blob_open(
    _db: *mut sqlite3,
    _dbname: *const c_char,
    _table: *const c_char,
    _column: *const c_char,
    _row: i64,
    _flags: c_int,
    _blob: *mut *mut c_void,
) -> c_int {
    SQLITE_ERROR
}
#[no_mangle]
pub extern "C" fn sqlite3_blob_close(_blob: *mut c_void) -> c_int {
    SQLITE_OK
}
#[no_mangle]
pub extern "C" fn sqlite3_blob_bytes(_blob: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub extern "C" fn sqlite3_blob_read(
    _blob: *mut c_void,
    _buf: *mut c_void,
    _n: c_int,
    _off: c_int,
) -> c_int {
    SQLITE_ERROR
}
#[no_mangle]
pub extern "C" fn sqlite3_blob_write(
    _blob: *mut c_void,
    _buf: *const c_void,
    _n: c_int,
    _off: c_int,
) -> c_int {
    SQLITE_ERROR
}

// ---- online backup (unsupported) ----

#[no_mangle]
pub extern "C" fn sqlite3_backup_init(
    _dest: *mut sqlite3,
    _destname: *const c_char,
    _src: *mut sqlite3,
    _srcname: *const c_char,
) -> *mut c_void {
    std::ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn sqlite3_backup_step(_b: *mut c_void, _n: c_int) -> c_int {
    SQLITE_ERROR
}
#[no_mangle]
pub extern "C" fn sqlite3_backup_finish(_b: *mut c_void) -> c_int {
    SQLITE_OK
}
#[no_mangle]
pub extern "C" fn sqlite3_backup_remaining(_b: *mut c_void) -> c_int {
    0
}
#[no_mangle]
pub extern "C" fn sqlite3_backup_pagecount(_b: *mut c_void) -> c_int {
    0
}

// ---- serialize / deserialize (unsupported) ----

#[no_mangle]
pub extern "C" fn sqlite3_serialize(
    _db: *mut sqlite3,
    _schema: *const c_char,
    _size: *mut i64,
    _flags: c_int,
) -> *mut u8 {
    std::ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn sqlite3_deserialize(
    _db: *mut sqlite3,
    _schema: *const c_char,
    _data: *mut u8,
    _sz: i64,
    _bufsz: i64,
    _flags: c_int,
) -> c_int {
    SQLITE_ERROR
}
