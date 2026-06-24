//! End-to-end SQL compatibility tests.

use std::sync::Arc;

use squrust_core::StorageEngine;
use squrust_sql::{SqlEngine, Value};

async fn engine() -> Arc<SqlEngine> {
    let storage = StorageEngine::open_memory().unwrap();
    SqlEngine::new(storage).await.unwrap()
}

async fn rows(eng: &SqlEngine, sql: &str, params: &[Value]) -> Vec<Vec<Value>> {
    let mut exec = eng.query(sql, params).await.unwrap();
    let mut out = Vec::new();
    while let Some(row) = exec.next().await.unwrap() {
        out.push(row.values);
    }
    out
}

fn ints(rows: &[Vec<Value>], col: usize) -> Vec<i64> {
    rows.iter().map(|r| r[col].as_i64().unwrap()).collect()
}

#[tokio::test]
async fn create_insert_select() {
    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")
        .await
        .unwrap();
    let n = eng
        .execute(
            "INSERT INTO users(id, name, age) VALUES (1, 'alice', 30), (2, 'bob', 25)",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(n, 2);

    let r = rows(&eng, "SELECT id, name, age FROM users", &[]).await;
    assert_eq!(r.len(), 2);
    assert_eq!(r[0][1], Value::Text("alice".into()));
    assert_eq!(r[1][2], Value::Integer(25));
}

#[tokio::test]
async fn where_order_limit() {
    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER)")
        .await
        .unwrap();
    for i in 1..=10 {
        eng.execute("INSERT INTO t(v) VALUES (?)", &[Value::Integer(i * 10)])
            .await
            .unwrap();
    }

    let r = rows(&eng, "SELECT v FROM t WHERE v > 50 ORDER BY v DESC LIMIT 3", &[]).await;
    assert_eq!(ints(&r, 0), vec![100, 90, 80]);

    let r = rows(&eng, "SELECT v FROM t ORDER BY v ASC LIMIT 2 OFFSET 2", &[]).await;
    assert_eq!(ints(&r, 0), vec![30, 40]);
}

#[tokio::test]
async fn update_and_delete() {
    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER)")
        .await
        .unwrap();
    eng.execute("INSERT INTO t(id, v) VALUES (1,1),(2,2),(3,3)", &[])
        .await
        .unwrap();

    let n = eng
        .execute("UPDATE t SET v = v * 100 WHERE id >= 2", &[])
        .await
        .unwrap();
    assert_eq!(n, 2);
    let r = rows(&eng, "SELECT v FROM t ORDER BY id", &[]).await;
    assert_eq!(ints(&r, 0), vec![1, 200, 300]);

    let n = eng.execute("DELETE FROM t WHERE v > 100", &[]).await.unwrap();
    assert_eq!(n, 2);
    let r = rows(&eng, "SELECT id FROM t", &[]).await;
    assert_eq!(ints(&r, 0), vec![1]);
}

#[tokio::test]
async fn aggregates_and_group_by() {
    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE sales(id INTEGER PRIMARY KEY, region TEXT, amount INTEGER)")
        .await
        .unwrap();
    eng.execute(
        "INSERT INTO sales(region, amount) VALUES ('west', 10), ('west', 20), ('east', 5)",
        &[],
    )
    .await
    .unwrap();

    let r = rows(&eng, "SELECT COUNT(*), SUM(amount), AVG(amount) FROM sales", &[]).await;
    assert_eq!(r[0][0], Value::Integer(3));
    assert_eq!(r[0][1], Value::Integer(35));

    let r = rows(
        &eng,
        "SELECT region, SUM(amount) FROM sales GROUP BY region ORDER BY region",
        &[],
    )
    .await;
    assert_eq!(r.len(), 2);
    assert_eq!(r[0][0], Value::Text("east".into()));
    assert_eq!(r[0][1], Value::Integer(5));
    assert_eq!(r[1][0], Value::Text("west".into()));
    assert_eq!(r[1][1], Value::Integer(30));
}

#[tokio::test]
async fn nulls_and_types() {
    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE t(id INTEGER PRIMARY KEY, a TEXT, b REAL)")
        .await
        .unwrap();
    eng.execute("INSERT INTO t(a, b) VALUES (NULL, 1.5), ('x', NULL)", &[])
        .await
        .unwrap();

    let r = rows(&eng, "SELECT a, b FROM t WHERE a IS NULL", &[]).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0][1], Value::Real(1.5));

    let r = rows(&eng, "SELECT a FROM t WHERE a IS NOT NULL", &[]).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0][0], Value::Text("x".into()));
}

#[tokio::test]
async fn inner_join() {
    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE u(id INTEGER PRIMARY KEY, name TEXT)")
        .await
        .unwrap();
    eng.execute_ddl("CREATE TABLE o(id INTEGER PRIMARY KEY, uid INTEGER, total INTEGER)")
        .await
        .unwrap();
    eng.execute("INSERT INTO u(id, name) VALUES (1,'a'),(2,'b')", &[])
        .await
        .unwrap();
    eng.execute(
        "INSERT INTO o(id, uid, total) VALUES (1,1,100),(2,1,50),(3,2,70)",
        &[],
    )
    .await
    .unwrap();

    let r = rows(
        &eng,
        "SELECT u.name, o.total FROM u JOIN o ON u.id = o.uid ORDER BY o.total",
        &[],
    )
    .await;
    assert_eq!(r.len(), 3);
    assert_eq!(r[0][0], Value::Text("a".into()));
    assert_eq!(r[0][1], Value::Integer(50));
    assert_eq!(r[2][1], Value::Integer(100));
}

#[tokio::test]
async fn expressions_and_functions() {
    let eng = engine().await;
    let r = rows(&eng, "SELECT 1 + 2 * 3", &[]).await;
    assert_eq!(r[0][0], Value::Integer(7));

    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)")
        .await
        .unwrap();
    eng.execute("INSERT INTO t(name) VALUES ('Hello')", &[])
        .await
        .unwrap();
    let r = rows(&eng, "SELECT UPPER(name), LENGTH(name) FROM t", &[]).await;
    assert_eq!(r[0][0], Value::Text("HELLO".into()));
    assert_eq!(r[0][1], Value::Integer(5));
}

#[tokio::test]
async fn insert_or_replace() {
    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
        .await
        .unwrap();
    eng.execute("INSERT INTO t(id, v) VALUES (1, 'first')", &[])
        .await
        .unwrap();
    eng.execute("INSERT OR REPLACE INTO t(id, v) VALUES (1, 'second')", &[])
        .await
        .unwrap();
    let r = rows(&eng, "SELECT v FROM t WHERE id = 1", &[]).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0][0], Value::Text("second".into()));
}

#[tokio::test]
async fn duplicate_pk_rejected() {
    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
        .await
        .unwrap();
    eng.execute("INSERT INTO t(id, v) VALUES (1, 'a')", &[])
        .await
        .unwrap();
    let err = eng
        .execute("INSERT INTO t(id, v) VALUES (1, 'b')", &[])
        .await;
    assert!(err.is_err(), "duplicate primary key must be rejected");
}

#[tokio::test]
async fn persistence_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.db");
    {
        let storage = StorageEngine::open(&path).unwrap();
        let eng = SqlEngine::new(storage).await.unwrap();
        eng.execute_ddl("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
            .await
            .unwrap();
        eng.execute("INSERT INTO t(id, v) VALUES (1, 'persisted')", &[])
            .await
            .unwrap();
        eng.storage().checkpoint().unwrap();
        eng.storage().sync().unwrap();
    }
    let storage = StorageEngine::open(&path).unwrap();
    let eng = SqlEngine::new(storage).await.unwrap();
    let r = rows(&eng, "SELECT v FROM t WHERE id = 1", &[]).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0][0], Value::Text("persisted".into()));
    assert_eq!(eng.table_names(), vec!["t".to_string()]);
}

#[tokio::test]
async fn type_affinity_matches_sqlite() {
    let eng = engine().await;
    eng.execute_ddl("CREATE TABLE t(a INTEGER, b TEXT, c REAL, d)")
        .await
        .unwrap();
    eng.execute("INSERT INTO t VALUES('123','456',789,3.5)", &[])
        .await
        .unwrap();
    let r = rows(
        &eng,
        "SELECT typeof(a), typeof(b), typeof(c), typeof(d) FROM t",
        &[],
    )
    .await;
    assert_eq!(
        r[0],
        vec![
            Value::Text("integer".into()),
            Value::Text("text".into()),
            Value::Text("real".into()),
            Value::Text("real".into()),
        ]
    );

    // A typeless column has NONE affinity: an integer stays an integer.
    let eng2 = engine().await;
    eng2.execute_ddl("CREATE TABLE u(x)").await.unwrap();
    eng2.execute("INSERT INTO u VALUES(1)", &[]).await.unwrap();
    let r = rows(&eng2, "SELECT typeof(x), x FROM u", &[]).await;
    assert_eq!(r[0][0], Value::Text("integer".into()));
    assert_eq!(r[0][1], Value::Integer(1));

    // INTEGER affinity keeps a fractional real as real (no truncation).
    let eng3 = engine().await;
    eng3.execute_ddl("CREATE TABLE w(x INTEGER)").await.unwrap();
    eng3.execute("INSERT INTO w VALUES(3.5)", &[]).await.unwrap();
    let r = rows(&eng3, "SELECT x, typeof(x) FROM w", &[]).await;
    assert_eq!(r[0][0], Value::Real(3.5));
    assert_eq!(r[0][1], Value::Text("real".into()));
}

#[tokio::test]
async fn cast_and_scalar_functions() {
    let eng = engine().await;
    let r = rows(
        &eng,
        "SELECT CAST(3.7 AS INTEGER), CAST('3.5abc' AS INTEGER), CAST(5 AS REAL), \
         substr('hello world',1,5), substr('hello',-3), replace('a.b.c','.','-'), \
         trim('  hi  '), ltrim('xxhi','x'), instr('abcdef','cd'), hex('AB'), \
         char(72,105), unicode('A'), nullif(5,5), nullif(5,6), sign(-3.2)",
        &[],
    )
    .await;
    let row = &r[0];
    assert_eq!(row[0], Value::Integer(3));
    assert_eq!(row[1], Value::Integer(3));
    assert_eq!(row[2], Value::Real(5.0));
    assert_eq!(row[3], Value::Text("hello".into()));
    assert_eq!(row[4], Value::Text("llo".into()));
    assert_eq!(row[5], Value::Text("a-b-c".into()));
    assert_eq!(row[6], Value::Text("hi".into()));
    assert_eq!(row[7], Value::Text("hi".into()));
    assert_eq!(row[8], Value::Integer(3));
    assert_eq!(row[9], Value::Text("4142".into()));
    assert_eq!(row[10], Value::Text("Hi".into()));
    assert_eq!(row[11], Value::Integer(65));
    assert!(row[12].is_null());
    assert_eq!(row[13], Value::Integer(5));
    assert_eq!(row[14], Value::Integer(-1));
}

#[tokio::test]
async fn case_and_integer_division_and_blob_literals() {
    let eng = engine().await;

    // Integer division truncates (10/3 == 3), like SQLite.
    let r = rows(&eng, "SELECT 10/3, 10%3, 10.0/4", &[]).await;
    assert_eq!(r[0][0], Value::Integer(3));
    assert_eq!(r[0][1], Value::Integer(1));
    assert_eq!(r[0][2], Value::Real(2.5));

    // Searched and simple CASE.
    let r = rows(
        &eng,
        "SELECT CASE WHEN 1 THEN 'yes' ELSE 'no' END, \
                CASE 2 WHEN 1 THEN 'a' WHEN 2 THEN 'b' ELSE 'c' END, \
                CASE WHEN 0 THEN 'x' END",
        &[],
    )
    .await;
    assert_eq!(r[0][0], Value::Text("yes".into()));
    assert_eq!(r[0][1], Value::Text("b".into()));
    assert!(r[0][2].is_null(), "CASE with no match and no ELSE is NULL");

    // Blob hex literal.
    let r = rows(&eng, "SELECT x'48656c6c6f', typeof(x'00')", &[]).await;
    assert_eq!(r[0][0], Value::Blob(b"Hello".to_vec()));
    assert_eq!(r[0][1], Value::Text("blob".into()));
}
