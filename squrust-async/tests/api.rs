//! Integration tests for the async API.

use futures::StreamExt;
use squrust_async::{Migration, SqurustConnection, SqurustPool};

#[tokio::test]
async fn typed_fetch_all_and_one() {
    let conn = SqurustConnection::open_memory().await.unwrap();
    conn.execute(
        "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, age INTEGER)",
        (),
    )
    .await
    .unwrap();
    for i in 1..=1000i64 {
        conn.execute(
            "INSERT INTO users(name, age) VALUES (?, ?)",
            (format!("user{i}"), i),
        )
        .await
        .unwrap();
    }

    // Tuple row mapping.
    let rows: Vec<(i64, String, i64)> = conn
        .query("SELECT id, name, age FROM users WHERE age <= 3 ORDER BY id")
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], (1, "user1".to_string(), 1));

    // Scalar fetch_one.
    let count: i64 = conn
        .query("SELECT COUNT(*) FROM users")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(count, 1000);

    // fetch_optional.
    let missing: Option<i64> = conn
        .query("SELECT id FROM users WHERE age = ?")
        .bind(99999i64)
        .fetch_optional()
        .await
        .unwrap();
    assert_eq!(missing, None);
}

#[tokio::test]
async fn streaming_rows() {
    let conn = SqurustConnection::open_memory().await.unwrap();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v INTEGER)", ())
        .await
        .unwrap();
    for i in 1..=50i64 {
        conn.execute("INSERT INTO t(v) VALUES (?)", (i * 2,))
            .await
            .unwrap();
    }

    let mut stream = conn
        .query("SELECT v FROM t ORDER BY v")
        .fetch_stream::<i64>();
    let mut sum = 0i64;
    let mut n = 0;
    while let Some(item) = stream.next().await {
        sum += item.unwrap();
        n += 1;
    }
    assert_eq!(n, 50);
    assert_eq!(sum, (1..=50i64).map(|i| i * 2).sum::<i64>());
}

#[tokio::test]
async fn transaction_commit_and_rollback() {
    let conn = SqurustConnection::open_memory().await.unwrap();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", ())
        .await
        .unwrap();

    // Rollback: changes discarded.
    {
        let tx = conn.begin().await.unwrap();
        tx.execute("INSERT INTO t(id, v) VALUES (1, 'rolled')", ())
            .await
            .unwrap();
        // Reads inside the transaction see the pending write.
        let inside: Vec<(i64, String)> = tx.fetch_all("SELECT id, v FROM t", ()).await.unwrap();
        assert_eq!(inside.len(), 1);
        tx.rollback().await;
    }
    let after: Vec<i64> = conn.query("SELECT id FROM t").fetch_all().await.unwrap();
    assert!(after.is_empty(), "rolled-back insert must not persist");

    // Commit: changes persist.
    {
        let tx = conn.begin().await.unwrap();
        tx.execute("INSERT INTO t(id, v) VALUES (2, 'kept')", ())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let kept: Vec<(i64, String)> = conn
        .query("SELECT id, v FROM t")
        .fetch_all()
        .await
        .unwrap();
    assert_eq!(kept, vec![(2, "kept".to_string())]);
}

#[tokio::test]
async fn migrations_apply_once() {
    let conn = SqurustConnection::open_memory().await.unwrap();
    let migrations = [
        Migration {
            version: 1,
            description: "create posts",
            sql: "CREATE TABLE posts(id INTEGER PRIMARY KEY, title TEXT)",
        },
        Migration {
            version: 2,
            description: "seed posts",
            sql: "INSERT INTO posts(id, title) VALUES (1, 'hello')",
        },
    ];
    conn.migrate(&migrations).await.unwrap();
    // Running again is a no-op (no duplicate seed).
    conn.migrate(&migrations).await.unwrap();

    let titles: Vec<String> = conn.query("SELECT title FROM posts").fetch_all().await.unwrap();
    assert_eq!(titles, vec!["hello".to_string()]);

    let applied: i64 = conn
        .query("SELECT COUNT(*) FROM _squrust_migrations")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(applied, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_concurrent_writes() {
    let pool = SqurustPool::open_memory(8).await.unwrap();
    {
        let c = pool.get().await.unwrap();
        c.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, who INTEGER)", ())
            .await
            .unwrap();
    }

    let mut handles = Vec::new();
    for who in 0..20i64 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let c = pool.get().await.unwrap();
            for _ in 0..10 {
                c.execute("INSERT INTO t(who) VALUES (?)", (who,))
                    .await
                    .unwrap();
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let c = pool.get().await.unwrap();
    let total: i64 = c
        .query("SELECT COUNT(*) FROM t")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(total, 200, "20 tasks * 10 inserts");
}
