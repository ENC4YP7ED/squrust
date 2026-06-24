//! Per-connection and per-statement state living behind the opaque C handles.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Arc;

use squrust_core::WriteTx;
use squrust_sql::{ColumnInfo, ReadSource, SqlEngine, SqlError, Value};

use crate::constants::*;
use crate::types::{sqlite3, sqlite3_stmt};

/// State behind a `sqlite3 *` handle.
pub struct ConnectionState {
    pub engine: Arc<SqlEngine>,
    /// The in-flight explicit transaction, if `BEGIN` is active.
    pub tx: Option<Arc<WriteTx>>,
    pub last_error: Option<CString>,
    pub last_errcode: std::os::raw::c_int,
}

impl ConnectionState {
    pub fn set_error(&mut self, code: std::os::raw::c_int, msg: impl Into<String>) {
        self.last_errcode = code;
        self.last_error = CString::new(msg.into()).ok();
    }

    pub fn clear_error(&mut self) {
        self.last_errcode = SQLITE_OK;
        self.last_error = None;
    }
}

/// State behind a `sqlite3_stmt *` handle.
pub struct StmtState {
    pub engine: Arc<SqlEngine>,
    /// The owning connection, used to surface errors via `sqlite3_errmsg`.
    pub db: *mut ConnectionState,
    pub sql: String,
    pub sql_cstr: CString,
    pub params: Vec<Value>,
    pub executed: bool,
    pub is_query: bool,
    pub columns: Vec<CString>,
    /// Declared column types (for `sqlite3_column_decltype`), parallel to `columns`.
    pub decltypes: Vec<Option<CString>>,
    pub rows: Vec<Vec<Value>>,
    pub cursor: usize,
    pub current: Option<usize>,
    /// True between a `step` that returned a row and the next `step`/`reset`
    /// (mirrors `sqlite3_stmt_busy`). CPython relies on this to decide whether a
    /// statement still has rows to yield.
    pub busy: bool,
    /// Caches keeping returned text/blob pointers valid until the next step.
    pub text_cache: Vec<Option<CString>>,
    pub blob_cache: Vec<Option<Vec<u8>>>,
}

impl StmtState {
    pub fn new(engine: Arc<SqlEngine>, db: *mut ConnectionState, sql: String) -> Self {
        let sql_cstr = CString::new(sql.as_str()).unwrap_or_default();
        StmtState {
            engine,
            db,
            sql,
            sql_cstr,
            params: Vec::new(),
            executed: false,
            is_query: false,
            columns: Vec::new(),
            decltypes: Vec::new(),
            rows: Vec::new(),
            cursor: 0,
            current: None,
            busy: false,
            text_cache: Vec::new(),
            blob_cache: Vec::new(),
        }
    }

    pub fn ensure_param(&mut self, one_based_col: i32) -> bool {
        if one_based_col < 1 {
            return false;
        }
        let idx = one_based_col as usize;
        if self.params.len() < idx {
            self.params.resize(idx, Value::Null);
        }
        true
    }

    pub fn bind(&mut self, one_based_col: i32, value: Value) -> i32 {
        if !self.ensure_param(one_based_col) {
            return SQLITE_RANGE;
        }
        self.params[one_based_col as usize - 1] = value;
        SQLITE_OK
    }

    fn run(&mut self) -> Result<(), SqlError> {
        let kw = first_keyword(&self.sql);
        // Safety: the connection outlives its statements.
        let db = unsafe { &mut *self.db };
        match kw.as_str() {
            "BEGIN" | "SAVEPOINT" => {
                self.is_query = false;
                if db.tx.is_none() {
                    db.tx = Some(Arc::new(self.engine.storage().begin_write()));
                }
            }
            "COMMIT" | "END" | "RELEASE" => {
                self.is_query = false;
                if let Some(tx) = db.tx.take() {
                    tx.commit()?;
                }
            }
            "ROLLBACK" => {
                self.is_query = false;
                if let Some(tx) = db.tx.take() {
                    tx.rollback();
                }
            }
            "EXPLAIN" => {
                // No-op that produces no rows.
                self.is_query = true;
                self.columns.clear();
                self.rows.clear();
            }
            "SELECT" | "WITH" | "VALUES" | "PRAGMA" => {
                self.is_query = true;
                // Inside a transaction, read its own uncommitted writes.
                let source: ReadSource = match &db.tx {
                    Some(tx) => tx.clone(),
                    None => Arc::new(self.engine.storage().begin_read()),
                };
                let mut exec = self.engine.build_query(source, &self.sql, &self.params)?;
                self.set_columns(exec.columns());
                let rows = block_on(async move {
                    let mut rows = Vec::new();
                    while let Some(r) = exec.next().await? {
                        rows.push(r.values);
                    }
                    Ok::<_, SqlError>(rows)
                })?;
                self.rows = rows;
            }
            _ => {
                self.is_query = false;
                let is_dml = matches!(kw.as_str(), "INSERT" | "UPDATE" | "DELETE");
                match (&db.tx, is_dml) {
                    (Some(tx), true) => {
                        block_on(self.engine.execute_on(tx, &self.sql, &self.params))?;
                    }
                    _ => {
                        block_on(self.engine.execute(&self.sql, &self.params))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Advance the statement; returns `SQLITE_ROW`, `SQLITE_DONE` or an error.
    pub fn step(&mut self) -> i32 {
        if !self.executed {
            self.executed = true;
            if let Err(e) = self.run() {
                // Map constraint violations to SQLITE_CONSTRAINT so callers
                // (e.g. CPython) raise IntegrityError rather than a generic error.
                let code = if matches!(e, SqlError::Constraint(_)) {
                    SQLITE_CONSTRAINT
                } else {
                    SQLITE_ERROR
                };
                self.set_db_error(code, e.to_string());
                return code;
            }
        }
        let rc = if self.is_query && self.cursor < self.rows.len() {
            self.current = Some(self.cursor);
            self.cursor += 1;
            let n = self.columns.len().max(self.cur_row_len());
            self.text_cache = vec![None; n];
            self.blob_cache = vec![None; n];
            SQLITE_ROW
        } else {
            SQLITE_DONE
        };
        // A statement is "busy" while positioned at a row; callers (CPython's
        // _sqlite3) use this to decide whether more rows remain.
        self.busy = rc == SQLITE_ROW;
        rc
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    fn cur_row_len(&self) -> usize {
        self.current
            .and_then(|i| self.rows.get(i))
            .map(|r| r.len())
            .unwrap_or(0)
    }

    pub fn reset(&mut self) {
        self.executed = false;
        self.is_query = false;
        self.rows.clear();
        self.columns.clear();
        self.decltypes.clear();
        self.cursor = 0;
        self.current = None;
        self.busy = false;
        self.text_cache.clear();
        self.blob_cache.clear();
    }

    /// Cache column names and declared types from a plan's column metadata.
    fn set_columns(&mut self, cols: &[ColumnInfo]) {
        self.columns = cols
            .iter()
            .map(|c| CString::new(c.name.as_str()).unwrap_or_default())
            .collect();
        self.decltypes = cols
            .iter()
            .map(|c| {
                c.decl_type
                    .as_deref()
                    .and_then(|d| CString::new(d).ok())
            })
            .collect();
    }

    pub fn decltype_ptr(&self, col: i32) -> *const std::os::raw::c_char {
        if col < 0 {
            return std::ptr::null();
        }
        match self.decltypes.get(col as usize) {
            Some(Some(cs)) => cs.as_ptr(),
            _ => std::ptr::null(),
        }
    }

    pub fn clear_bindings(&mut self) {
        self.params.clear();
    }

    pub fn value_at(&self, col: i32) -> Option<&Value> {
        let row = self.rows.get(self.current?)?;
        row.get(col as usize)
    }

    pub fn column_count(&self) -> i32 {
        self.columns.len() as i32
    }

    /// Populate column names without executing, so `sqlite3_column_count` /
    /// `sqlite3_column_name` work right after `prepare`.
    pub fn ensure_columns(&mut self) {
        if self.columns.is_empty() && !self.executed {
            if let Ok(cols) = self.engine.describe(&self.sql) {
                self.set_columns(&cols);
            }
        }
    }

    fn set_db_error(&self, code: i32, msg: String) {
        if !self.db.is_null() {
            // Safety: the connection outlives its statements (caller must
            // finalize before close).
            unsafe { (*self.db).set_error(code, msg) };
        }
    }
}

/// Drive a future to completion with a no-op waker.
///
/// Squrust's `SqlEngine` futures do pure synchronous work wrapped in `async`;
/// they never return `Poll::Pending`, so this trivial executor is sufficient —
/// and it avoids running a Tokio reactor across the CPython `_sqlite3` GIL
/// release boundary, which corrupted call results.
pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::hint::spin_loop();
    }
}

/// Map a [`Value`] to a `SQLITE_*` column type code.
pub fn value_type_code(v: &Value) -> i32 {
    match v {
        Value::Null => SQLITE_NULL,
        Value::Integer(_) | Value::Boolean(_) => SQLITE_INTEGER,
        Value::Real(_) => SQLITE_FLOAT,
        Value::Text(_) | Value::Json(_) => SQLITE_TEXT,
        Value::Blob(_) => SQLITE_BLOB,
    }
}

// ---- pointer helpers ----

/// Reborrow a `sqlite3 *` as a `ConnectionState`.
///
/// # Safety
/// `db` must be a pointer returned by `sqlite3_open*` and not yet closed.
pub unsafe fn conn<'a>(db: *mut sqlite3) -> Option<&'a mut ConnectionState> {
    (db as *mut ConnectionState).as_mut()
}

/// Reborrow a `sqlite3_stmt *` as a `StmtState`.
///
/// # Safety
/// `stmt` must be a pointer returned by `sqlite3_prepare_v2` and not finalized.
pub unsafe fn stmt<'a>(stmt: *mut sqlite3_stmt) -> Option<&'a mut StmtState> {
    (stmt as *mut StmtState).as_mut()
}

/// Convert a C string (optionally length-limited) to an owned Rust `String`.
///
/// # Safety
/// `ptr` must be NUL-terminated (when `nbyte < 0`) or have at least `nbyte`
/// readable bytes.
pub unsafe fn c_to_string(ptr: *const c_char, nbyte: i32) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    if nbyte < 0 {
        Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
    } else {
        let bytes = std::slice::from_raw_parts(ptr as *const u8, nbyte as usize);
        // Stop at an embedded NUL if present.
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

fn first_keyword(sql: &str) -> String {
    sql.trim_start()
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}
