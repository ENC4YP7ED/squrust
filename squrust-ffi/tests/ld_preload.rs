//! Verifies that Python's stdlib `sqlite3` works against libsqurust via
//! `LD_PRELOAD`. Skips (rather than fails) when `python3` or the built cdylib
//! are unavailable, e.g. if `cargo build` hasn't produced the `.so` yet.

use std::path::PathBuf;
use std::process::Command;

fn find_dylib() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest.parent()?;
    for profile in ["debug", "release"] {
        let p = workspace.join("target").join(profile).join("libsqurust.so");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn have_python() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn python_sqlite3_via_ld_preload() {
    let Some(dylib) = find_dylib() else {
        eprintln!("skipping: libsqurust.so not built (run `cargo build` first)");
        return;
    };
    if !have_python() {
        eprintln!("skipping: python3 not available");
        return;
    }

    let script = r#"
import sqlite3
con = sqlite3.connect(":memory:")
con.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")
con.executemany("INSERT INTO t(name,age) VALUES(?,?)",
                [("alice",30),("bob",25),("carol",41)])
con.commit()
rows = con.execute("SELECT name,age FROM t WHERE age>? ORDER BY age", (26,)).fetchall()
assert rows == [("alice",30),("carol",41)], rows
assert con.execute("SELECT COUNT(*) FROM t").fetchone()[0] == 3
# transaction rollback
con.execute("UPDATE t SET age=0 WHERE name='alice'")
con.rollback()
assert con.execute("SELECT age FROM t WHERE name='alice'").fetchone()[0] == 30
con.close()
print("PYTHON_LD_PRELOAD_OK")
"#;

    let out = Command::new("python3")
        .arg("-c")
        .arg(script)
        .env("LD_PRELOAD", &dylib)
        .output()
        .expect("run python3");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("PYTHON_LD_PRELOAD_OK"),
        "python sqlite3 over LD_PRELOAD failed:\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
