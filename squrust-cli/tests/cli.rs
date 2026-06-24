//! End-to-end tests that run the compiled `sq` binary.

use std::process::Command;

fn sq(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_sq"))
        .args(args)
        .output()
        .expect("run sq");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn one_shot_table_output() {
    let (stdout, _stderr, ok) = sq(&[
        ":memory:",
        "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT); \
         INSERT INTO t VALUES (1,'alice'),(2,'bob'); \
         SELECT * FROM t ORDER BY id",
    ]);
    assert!(ok, "sq exited non-zero");
    assert!(stdout.contains("alice"), "output: {stdout}");
    assert!(stdout.contains("bob"), "output: {stdout}");
    assert!(stdout.contains("id"), "header present");
}

#[test]
fn csv_mode() {
    let (stdout, _e, ok) = sq(&[
        "--mode",
        "csv",
        ":memory:",
        "CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (3),(1),(2); \
         SELECT x FROM t ORDER BY x",
    ]);
    assert!(ok);
    assert_eq!(stdout.trim(), "x\n1\n2\n3");
}

#[test]
fn json_mode() {
    let (stdout, _e, ok) = sq(&[
        "--mode",
        "json",
        ":memory:",
        "CREATE TABLE t(a INTEGER, b TEXT); INSERT INTO t VALUES (1,'x'); SELECT * FROM t",
    ]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v[0]["a"], 1);
    assert_eq!(v[0]["b"], "x");
}
