//! Exercise the C ABI from Rust (the crate is also built as an rlib).

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use squrust::bind::{sqlite3_bind_int64, sqlite3_bind_text};
use squrust::column::{sqlite3_column_count, sqlite3_column_int64, sqlite3_column_text};
use squrust::exec::sqlite3_exec;
use squrust::meta::sqlite3_last_insert_rowid;
use squrust::open::{sqlite3_close, sqlite3_open};
use squrust::prepare::{sqlite3_finalize, sqlite3_prepare_v2};
use squrust::step::sqlite3_step;
use squrust::{SQLITE_DONE, SQLITE_OK, SQLITE_ROW, sqlite3, sqlite3_stmt};

unsafe fn prepare(db: *mut sqlite3, sql: &str) -> *mut sqlite3_stmt {
    let csql = CString::new(sql).unwrap();
    let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
    let rc = sqlite3_prepare_v2(db, csql.as_ptr(), -1, &mut stmt, ptr::null_mut());
    assert_eq!(rc, SQLITE_OK, "prepare `{sql}`");
    stmt
}

#[test]
fn full_ffi_roundtrip() {
    unsafe {
        let mut db: *mut sqlite3 = ptr::null_mut();
        let mem = CString::new(":memory:").unwrap();
        assert_eq!(sqlite3_open(mem.as_ptr(), &mut db), SQLITE_OK);
        assert!(!db.is_null());

        let ddl = CString::new("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, n INTEGER)").unwrap();
        assert_eq!(
            sqlite3_exec(db, ddl.as_ptr(), None, ptr::null_mut(), ptr::null_mut()),
            SQLITE_OK
        );

        // Insert two rows with bound parameters.
        let ins = prepare(db, "INSERT INTO t(name, n) VALUES (?, ?)");
        for (name, n) in [("a", 1i64), ("b", 2)] {
            let cname = CString::new(name).unwrap();
            sqlite3_bind_text(ins, 1, cname.as_ptr(), -1, None);
            sqlite3_bind_int64(ins, 2, n);
            assert_eq!(sqlite3_step(ins), SQLITE_DONE);
            // Reset for reuse.
            squrust::step::sqlite3_reset(ins);
            squrust::step::sqlite3_clear_bindings(ins);
        }
        sqlite3_finalize(ins);
        assert_eq!(sqlite3_last_insert_rowid(db), 2);

        // Read them back.
        let sel = prepare(db, "SELECT name, n FROM t ORDER BY n");
        assert_eq!(sqlite3_column_count(sel), 2);
        let mut collected = Vec::new();
        while sqlite3_step(sel) == SQLITE_ROW {
            let name_ptr: *const c_char = sqlite3_column_text(sel, 0) as *const c_char;
            let name = CStr::from_ptr(name_ptr).to_string_lossy().into_owned();
            let n = sqlite3_column_int64(sel, 1);
            collected.push((name, n));
        }
        sqlite3_finalize(sel);
        assert_eq!(
            collected,
            vec![("a".to_string(), 1), ("b".to_string(), 2)]
        );

        assert_eq!(sqlite3_close(db), SQLITE_OK);
    }
}
