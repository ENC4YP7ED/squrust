//! `sqlite3_open` family.

use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use squrust_core::StorageEngine;
use squrust_sql::SqlEngine;

use crate::constants::*;
use crate::state::{ConnectionState, block_on, c_to_string, conn};
use crate::types::sqlite3;

async fn build_engine(path: Option<&str>) -> Result<Arc<SqlEngine>, String> {
    let storage = match path {
        Some(p) => StorageEngine::open(Path::new(p)),
        None => StorageEngine::open_memory(),
    }
    .map_err(|e| e.to_string())?;
    SqlEngine::new(storage).await.map_err(|e| e.to_string())
}

/// # Safety
/// `ppDb` must be a valid out-pointer; `filename` a NUL-terminated string or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_open(filename: *const c_char, pp_db: *mut *mut sqlite3) -> c_int {
    sqlite3_open_v2(
        filename,
        pp_db,
        SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
        ptr::null(),
    )
}

/// # Safety
/// See [`sqlite3_open`]. `flags` and `z_vfs` are accepted but mostly ignored.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_open_v2(
    filename: *const c_char,
    pp_db: *mut *mut sqlite3,
    _flags: c_int,
    _z_vfs: *const c_char,
) -> c_int {
    if pp_db.is_null() {
        return SQLITE_MISUSE;
    }
    *pp_db = ptr::null_mut();

    let name = c_to_string(filename, -1).unwrap_or_default();
    let path = if name.is_empty() || name == ":memory:" {
        None
    } else {
        Some(name.as_str())
    };

    match block_on(build_engine(path)) {
        Ok(engine) => {
            let state = Box::new(ConnectionState {
                engine,
                tx: None,
                last_error: None,
                last_errcode: SQLITE_OK,
            });
            *pp_db = Box::into_raw(state) as *mut sqlite3;
            SQLITE_OK
        }
        Err(_) => SQLITE_CANTOPEN,
    }
}

/// # Safety
/// `db` must come from `sqlite3_open*` (or be NULL) and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_close(db: *mut sqlite3) -> c_int {
    if !db.is_null() {
        // Fold the WAL into the main file so a closed file database is a
        // complete, stock-sqlite3-readable .db (cross-process durability).
        if let Some(c) = conn(db) {
            let _ = c.engine.storage().checkpoint();
        }
        drop(Box::from_raw(db as *mut ConnectionState));
    }
    SQLITE_OK
}

/// # Safety
/// See [`sqlite3_close`].
#[no_mangle]
pub unsafe extern "C" fn sqlite3_close_v2(db: *mut sqlite3) -> c_int {
    sqlite3_close(db)
}
