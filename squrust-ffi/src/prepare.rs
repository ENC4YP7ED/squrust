//! `sqlite3_prepare_v2`, `sqlite3_finalize`, `sqlite3_sql`, `sqlite3_stmt_readonly`.

use std::os::raw::{c_char, c_int};
use std::ptr;

use crate::constants::*;
use crate::state::{StmtState, c_to_string, conn, stmt};
use crate::types::{sqlite3, sqlite3_stmt};

/// # Safety
/// `db` from `sqlite3_open*`; `pp_stmt` a valid out-pointer.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_prepare_v2(
    db: *mut sqlite3,
    sql: *const c_char,
    n_byte: c_int,
    pp_stmt: *mut *mut sqlite3_stmt,
    pz_tail: *mut *const c_char,
) -> c_int {
    let Some(c) = conn(db) else {
        return SQLITE_MISUSE;
    };
    if pp_stmt.is_null() {
        return SQLITE_MISUSE;
    }
    *pp_stmt = ptr::null_mut();

    let text = c_to_string(sql, n_byte).unwrap_or_default();

    // Validate by parsing; surface syntax errors at prepare time. PRAGMAs are
    // handled by a dedicated parser (they accept unquoted identifier arguments
    // the SQL grammar rejects), so they skip this check.
    if squrust_sql::pragma::try_parse(&text).is_none() {
        if let Err(e) = squrust_sql::parser::parse(&text) {
            c.set_error(SQLITE_ERROR, e.to_string());
            return SQLITE_ERROR;
        }
    }

    if !pz_tail.is_null() {
        // We consume the entire input; tail points at the terminating NUL.
        let consumed = if n_byte < 0 {
            text.len()
        } else {
            (n_byte as usize).min(text.len())
        };
        *pz_tail = sql.add(consumed);
    }

    let state = Box::new(StmtState::new(
        c.engine.clone(),
        db as *mut crate::state::ConnectionState,
        text,
    ));
    *pp_stmt = Box::into_raw(state) as *mut sqlite3_stmt;
    c.clear_error();
    SQLITE_OK
}

/// Legacy alias.
/// # Safety
/// See [`sqlite3_prepare_v2`].
#[no_mangle]
pub unsafe extern "C" fn sqlite3_prepare(
    db: *mut sqlite3,
    sql: *const c_char,
    n_byte: c_int,
    pp_stmt: *mut *mut sqlite3_stmt,
    pz_tail: *mut *const c_char,
) -> c_int {
    sqlite3_prepare_v2(db, sql, n_byte, pp_stmt, pz_tail)
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`, not previously finalized.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_finalize(s: *mut sqlite3_stmt) -> c_int {
    if !s.is_null() {
        drop(Box::from_raw(s as *mut StmtState));
    }
    SQLITE_OK
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_sql(s: *mut sqlite3_stmt) -> *const c_char {
    match stmt(s) {
        Some(st) => st.sql_cstr.as_ptr(),
        None => ptr::null(),
    }
}

/// # Safety
/// `s` from `sqlite3_prepare_v2`.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_stmt_readonly(s: *mut sqlite3_stmt) -> c_int {
    match stmt(s) {
        Some(st) => {
            let kw = st
                .sql
                .trim_start()
                .split(|c: char| c.is_whitespace() || c == '(')
                .next()
                .unwrap_or("")
                .to_ascii_uppercase();
            i32::from(matches!(kw.as_str(), "SELECT" | "WITH" | "VALUES" | "EXPLAIN" | "PRAGMA"))
        }
        None => 1,
    }
}
