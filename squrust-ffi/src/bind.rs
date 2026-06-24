//! `sqlite3_bind_*`.

use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

use squrust_sql::Value;

use crate::constants::*;
use crate::state::{c_to_string, stmt};
use crate::types::sqlite3_stmt;

type Destructor = Option<unsafe extern "C" fn(*mut c_void)>;

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_int(s: *mut sqlite3_stmt, col: c_int, val: c_int) -> c_int {
    bind(s, col, Value::Integer(val as i64))
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_int64(s: *mut sqlite3_stmt, col: c_int, val: i64) -> c_int {
    bind(s, col, Value::Integer(val))
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_double(s: *mut sqlite3_stmt, col: c_int, val: f64) -> c_int {
    bind(s, col, Value::Real(val))
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`; `val` valid for `n` bytes (or NUL-terminated if `n<0`).
#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_text(
    s: *mut sqlite3_stmt,
    col: c_int,
    val: *const c_char,
    n: c_int,
    _destructor: Destructor,
) -> c_int {
    let text = c_to_string(val, n).unwrap_or_default();
    bind(s, col, Value::Text(text))
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`; `val` valid for `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_blob(
    s: *mut sqlite3_stmt,
    col: c_int,
    val: *const c_void,
    n: c_int,
    _destructor: Destructor,
) -> c_int {
    let bytes = if val.is_null() || n < 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(val as *const u8, n as usize).to_vec()
    };
    bind(s, col, Value::Blob(bytes))
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_null(s: *mut sqlite3_stmt, col: c_int) -> c_int {
    bind(s, col, Value::Null)
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_zeroblob(
    s: *mut sqlite3_stmt,
    col: c_int,
    n: c_int,
) -> c_int {
    bind(s, col, Value::Blob(vec![0u8; n.max(0) as usize]))
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_parameter_count(s: *mut sqlite3_stmt) -> c_int {
    match stmt(s) {
        Some(st) => st.sql.matches('?').count() as c_int,
        None => 0,
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_parameter_name(
    _s: *mut sqlite3_stmt,
    _col: c_int,
) -> *const c_char {
    // Positional parameters have no name.
    ptr::null()
}

unsafe fn bind(s: *mut sqlite3_stmt, col: c_int, value: Value) -> c_int {
    match stmt(s) {
        Some(st) => st.bind(col, value),
        None => SQLITE_MISUSE,
    }
}
