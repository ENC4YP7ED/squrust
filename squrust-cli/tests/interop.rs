//! On-disk interoperability with stock `sqlite3`. These tests shell out to the
//! `sqlite3` binary; if it is not installed they are skipped.

use std::process::Command;

fn have_sqlite3() -> bool {
    Command::new("sqlite3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn sq(db: &str, sql: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_sq"))
        .args([db, sql])
        .output()
        .expect("run sq");
    assert!(out.status.success(), "sq failed: {:?}", out);
}

fn sqlite3(db: &str, sql: &str) -> String {
    let out = Command::new("sqlite3")
        .args([db, sql])
        .output()
        .expect("run sqlite3");
    assert!(
        out.status.success(),
        "sqlite3 failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn tmp(name: &str) -> String {
    let p = std::env::temp_dir().join(format!("squrust-interop-{}-{name}.db", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_file(format!("{}-squrust-wal", p.display()));
    p.to_string_lossy().into_owned()
}

#[test]
fn sqlite3_reads_squrust_file() {
    if !have_sqlite3() {
        eprintln!("skipping: sqlite3 not installed");
        return;
    }
    let db = tmp("write");
    sq(
        &db,
        "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, age INTEGER, score REAL);\
         INSERT INTO users(name,age,score) VALUES('alice',30,9.5),('bob',25,7.0),('carol',41,8.25)",
    );

    assert_eq!(
        sqlite3(&db, "SELECT id,name,age FROM users ORDER BY age").trim(),
        "2|bob|25\n1|alice|30\n3|carol|41"
    );
    assert_eq!(sqlite3(&db, "PRAGMA integrity_check").trim(), "ok");
    assert!(sqlite3(&db, ".schema").contains("CREATE TABLE users"));
    let _ = std::fs::remove_file(&db);
}

#[test]
fn squrust_reads_sqlite3_file() {
    if !have_sqlite3() {
        eprintln!("skipping: sqlite3 not installed");
        return;
    }
    let db = tmp("read");
    sqlite3(
        &db,
        "CREATE TABLE products(id INTEGER PRIMARY KEY, name TEXT, price REAL);\
         INSERT INTO products(name,price) VALUES('widget',9.99),('gadget',19.5);",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_sq"))
        .args([
            "--mode",
            "csv",
            &db,
            "SELECT id,name,price FROM products ORDER BY id",
        ])
        .output()
        .expect("run sq");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert_eq!(stdout.trim(), "id,name,price\n1,widget,9.99\n2,gadget,19.5");
    let _ = std::fs::remove_file(&db);
}
