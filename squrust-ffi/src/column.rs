//! `sqlite3_column_*`.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_uchar, c_void};
use std::ptr;

use squrust_sql::Value;

use crate::constants::*;
use crate::state::{StmtState, stmt, value_type_code};
use crate::types::sqlite3_stmt;

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_count(s: *mut sqlite3_stmt) -> c_int {
    match stmt(s) {
        Some(st) => {
            st.ensure_columns();
            st.column_count()
        }
        None => 0,
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_data_count(s: *mut sqlite3_stmt) -> c_int {
    match stmt(s) {
        Some(st) if st.current.is_some() => st.column_count(),
        _ => 0,
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_type(s: *mut sqlite3_stmt, col: c_int) -> c_int {
    match stmt(s).and_then(|st| st.value_at(col).cloned()) {
        Some(v) => value_type_code(&v),
        None => SQLITE_NULL,
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_name(s: *mut sqlite3_stmt, col: c_int) -> *const c_char {
    match stmt(s) {
        Some(st) => {
            st.ensure_columns();
            if col >= 0 && (col as usize) < st.columns.len() {
                st.columns[col as usize].as_ptr()
            } else {
                ptr::null()
            }
        }
        None => ptr::null(),
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_int(s: *mut sqlite3_stmt, col: c_int) -> c_int {
    sqlite3_column_int64(s, col) as c_int
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_int64(s: *mut sqlite3_stmt, col: c_int) -> i64 {
    match stmt(s).and_then(|st| st.value_at(col).cloned()) {
        Some(v) => v.as_i64().unwrap_or(0),
        None => 0,
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_double(s: *mut sqlite3_stmt, col: c_int) -> f64 {
    match stmt(s).and_then(|st| st.value_at(col).cloned()) {
        Some(v) => v.as_f64().unwrap_or(0.0),
        None => 0.0,
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`. The returned pointer is valid until the next
/// `sqlite3_step`/`sqlite3_reset`/`sqlite3_finalize`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_text(s: *mut sqlite3_stmt, col: c_int) -> *const c_uchar {
    let Some(st) = stmt(s) else {
        return ptr::null();
    };
    let value = st.value_at(col).cloned();
    match value {
        None | Some(Value::Null) => ptr::null(),
        Some(v) => {
            let cs = CString::new(v.to_display_string()).unwrap_or_default();
            let ptr = cs.as_ptr() as *const c_uchar;
            cache_text(st, col, cs);
            ptr
        }
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`. Pointer valid until the next step/reset/finalize.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_blob(s: *mut sqlite3_stmt, col: c_int) -> *const c_void {
    let Some(st) = stmt(s) else {
        return ptr::null();
    };
    let value = st.value_at(col).cloned();
    let bytes = match value {
        Some(Value::Blob(b)) => b,
        Some(Value::Text(t)) => t.into_bytes(),
        Some(Value::Null) | None => return ptr::null(),
        Some(other) => other.to_display_string().into_bytes(),
    };
    if bytes.is_empty() {
        return ptr::null();
    }
    let ptr = bytes.as_ptr() as *const c_void;
    cache_blob(st, col, bytes);
    ptr
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_bytes(s: *mut sqlite3_stmt, col: c_int) -> c_int {
    match stmt(s).and_then(|st| st.value_at(col).cloned()) {
        Some(Value::Blob(b)) => b.len() as c_int,
        Some(Value::Null) | None => 0,
        Some(v) => v.to_display_string().len() as c_int,
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_decltype(
    _s: *mut sqlite3_stmt,
    _col: c_int,
) -> *const c_char {
    ptr::null()
}

fn cache_text(st: &mut StmtState, col: c_int, cs: CString) {
    let idx = col.max(0) as usize;
    if st.text_cache.len() <= idx {
        st.text_cache.resize(idx + 1, None);
    }
    st.text_cache[idx] = Some(cs);
}

fn cache_blob(st: &mut StmtState, col: c_int, bytes: Vec<u8>) {
    let idx = col.max(0) as usize;
    if st.blob_cache.len() <= idx {
        st.blob_cache.resize(idx + 1, None);
    }
    st.blob_cache[idx] = Some(bytes);
}
