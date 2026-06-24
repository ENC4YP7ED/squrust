//! `sqlite3_step`, `sqlite3_reset`, `sqlite3_clear_bindings`.

use std::os::raw::c_int;

use crate::constants::*;
use crate::state::stmt;
use crate::types::sqlite3_stmt;

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_step(s: *mut sqlite3_stmt) -> c_int {
    match stmt(s) {
        Some(st) => st.step(),
        None => SQLITE_MISUSE,
    }
}

/// Whether the statement is positioned at a row (mirrors `sqlite3_stmt_busy`).
///
/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_stmt_busy(s: *mut sqlite3_stmt) -> c_int {
    match stmt(s) {
        Some(st) => st.is_busy() as c_int,
        None => 0,
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_reset(s: *mut sqlite3_stmt) -> c_int {
    match stmt(s) {
        Some(st) => {
            st.reset();
            SQLITE_OK
        }
        None => SQLITE_MISUSE,
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_clear_bindings(s: *mut sqlite3_stmt) -> c_int {
    match stmt(s) {
        Some(st) => {
            st.clear_bindings();
            SQLITE_OK
        }
        None => SQLITE_MISUSE,
    }
}
