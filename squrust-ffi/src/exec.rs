//! `sqlite3_exec`: parse, run, and invoke a callback per result row.

use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

use squrust_sql::{SqlError, Value};

use crate::constants::*;
use crate::mem::c_string_dup;
use crate::state::{block_on, c_to_string, conn};
use crate::types::sqlite3;

type Callback =
    Option<unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int>;

/// # Safety
/// `db` from `sqlite3_open*`; `sql` NUL-terminated; `errmsg` a valid out-pointer or NULL.
#[no_mangle]
pub unsafe extern "C" fn sqlite3_exec(
    db: *mut sqlite3,
    sql: *const c_char,
    callback: Callback,
    arg: *mut c_void,
    errmsg: *mut *mut c_char,
) -> c_int {
    if !errmsg.is_null() {
        *errmsg = ptr::null_mut();
    }
    let Some(c) = conn(db) else {
        return SQLITE_MISUSE;
    };
    let text = c_to_string(sql, -1).unwrap_or_default();

    let statements = match squrust_sql::parser::parse(&text) {
        Ok(s) => s,
        Err(e) => return fail(c, errmsg, e.to_string()),
    };

    let engine = c.engine.clone();

    for stmt in &statements {
        let stmt_sql = stmt.to_string();
        let kw = first_keyword(&stmt_sql);
        if matches!(kw.as_str(), "SELECT" | "WITH" | "VALUES") {
            let result = block_on(async {
                let mut exec = engine.query(&stmt_sql, &[]).await?;
                let cols: Vec<String> = exec.columns().iter().map(|c| c.name.clone()).collect();
                let mut rows = Vec::new();
                while let Some(r) = exec.next().await? {
                    rows.push(r.values);
                }
                Ok::<_, SqlError>((cols, rows))
            });
            let (cols, rows) = match result {
                Ok(v) => v,
                Err(e) => return fail(c, errmsg, e.to_string()),
            };
            if let Some(cb) = callback {
                let ncol = cols.len() as c_int;
                let mut names: Vec<*mut c_char> = cols.iter().map(|n| c_string_dup(n)).collect();
                for row in &rows {
                    let mut argv: Vec<*mut c_char> = row
                        .iter()
                        .map(|v| match v {
                            Value::Null => ptr::null_mut(),
                            other => c_string_dup(&other.to_display_string()),
                        })
                        .collect();
                    let rc = cb(arg, ncol, argv.as_mut_ptr(), names.as_mut_ptr());
                    for p in &argv {
                        if !p.is_null() {
                            libc::free(*p as *mut c_void);
                        }
                    }
                    if rc != 0 {
                        free_all(&names);
                        return SQLITE_ABORT;
                    }
                }
                free_all(&names);
            }
        } else {
            if let Err(e) = block_on(engine.execute(&stmt_sql, &[])) {
                return fail(c, errmsg, e.to_string());
            }
        }
    }
    c.clear_error();
    SQLITE_OK
}

unsafe fn free_all(ptrs: &[*mut c_char]) {
    for p in ptrs {
        if !p.is_null() {
            libc::free(*p as *mut c_void);
        }
    }
}

unsafe fn fail(
    c: &mut crate::state::ConnectionState,
    errmsg: *mut *mut c_char,
    msg: String,
) -> c_int {
    c.set_error(SQLITE_ERROR, msg.clone());
    if !errmsg.is_null() {
        *errmsg = c_string_dup(&msg);
    }
    SQLITE_ERROR
}

fn first_keyword(sql: &str) -> String {
    sql.trim_start()
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}
