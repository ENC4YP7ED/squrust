//! Blocking-API tests — note there is no `async` anywhere in this file.

use squrust_sync::{SyncConnection, SyncPool};

#[test]
fn crud_without_async() {
    let conn = SyncConnection::open_memory().unwrap();
    conn.execute(
        "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, age INTEGER)",
        (),
    )
    .unwrap();

    conn.execute(
        "INSERT INTO users(name, age) VALUES (?, ?)",
        ("alice", 30i64),
    )
    .unwrap();
    conn.execute("INSERT INTO users(name, age) VALUES (?, ?)", ("bob", 25i64))
        .unwrap();

    let rows: Vec<(i64, String, i64)> = conn
        .query("SELECT id, name, age FROM users ORDER BY age")
        .fetch_all()
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, "bob");
    assert_eq!(rows[1].1, "alice");

    let count: i64 = conn
        .query("SELECT COUNT(*) FROM users WHERE age > ?")
        .bind(26i64)
        .fetch_one()
        .unwrap();
    assert_eq!(count, 1);

    let n = conn
        .execute("UPDATE users SET age = age + 1 WHERE name = ?", ("bob",))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn transaction_blocking() {
    let conn = SyncConnection::open_memory().unwrap();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", ())
        .unwrap();

    let tx = conn.begin().unwrap();
    tx.execute("INSERT INTO t(id, v) VALUES (1, 'a')", ()).unwrap();
    tx.execute("INSERT INTO t(id, v) VALUES (2, 'b')", ()).unwrap();
    let inside: Vec<i64> = tx.fetch_all("SELECT id FROM t ORDER BY id", ()).unwrap();
    assert_eq!(inside, vec![1, 2]);
    tx.commit().unwrap();

    let after: Vec<i64> = conn.query("SELECT id FROM t ORDER BY id").fetch_all().unwrap();
    assert_eq!(after, vec![1, 2]);
}

#[test]
fn pool_blocking() {
    let pool = SyncPool::open_memory(4).unwrap();
    let c = pool.get().unwrap();
    c.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER)", ())
        .unwrap();
    c.execute("INSERT INTO t(v) VALUES (10),(20)", ()).unwrap();
    let vals: Vec<i64> = c.fetch_all("SELECT v FROM t ORDER BY v", ()).unwrap();
    assert_eq!(vals, vec![10, 20]);
}
