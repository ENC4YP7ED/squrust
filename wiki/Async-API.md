# Async API (`squrust-async`)

The primary, idiomatic Rust interface. Wraps the [[SQL Engine]] with async
ergonomics: a query builder, typed row mapping, streaming, transactions, a
connection pool, and migrations.

```toml
[dependencies]
squrust-async = { git = "https://github.com/ENC4YP7ED/squrust" }
tokio         = { version = "1", features = ["full"] }
futures       = "0.3"   # only if you use fetch_stream
```

## Connections

```rust
use squrust_async::SqurustConnection;

let conn = SqurustConnection::open("app.db").await?;   // file-backed
let conn = SqurustConnection::open_memory().await?;    // transient
```

`SqurustConnection` is `Clone` (it shares one engine), so you can hand clones to
many tasks.

## The `Query` builder

```rust
let q = conn.query("SELECT id, name FROM users WHERE age > ? AND city = ?")
    .bind(30)
    .bind("Berlin");
```

Run it one of several ways:

| Method | Returns |
|--------|---------|
| `fetch_all::<T>()` | `Vec<T>` |
| `fetch_one::<T>()` | `T` (errors if no row) |
| `fetch_optional::<T>()` | `Option<T>` |
| `fetch_stream::<T>()` | `RowStream<T>` (lazy) |
| `execute()` | `u64` rows affected |

`T` is anything implementing [`FromRow`](Derive-Macros): scalars (`i64`,
`String`, `Option<T>`, …), tuples up to 8, or your own `#[derive(FromRow)]`
structs.

```rust
// scalar
let n: i64 = conn.query("SELECT COUNT(*) FROM users").fetch_one().await?;

// tuple
let rows: Vec<(i64, String)> =
    conn.query("SELECT id, name FROM users ORDER BY id").fetch_all().await?;

// optional
let maybe: Option<String> =
    conn.query("SELECT name FROM users WHERE id = ?").bind(42).fetch_optional().await?;
```

### `execute` and parameter binding

`SqurustConnection::execute` and `Query` accept any `impl ToParams`: `()`,
`Vec<Value>`, slices, arrays, and tuples up to 12. (`Value` itself is
re-exported as `squrust_async::Value`.)

```rust
conn.execute("INSERT INTO users(name, age) VALUES (?, ?)", ("ada", 36)).await?;
conn.execute("DELETE FROM users WHERE age < ?", (18,)).await?;
let last = conn.last_insert_rowid();
let changed = conn.changes();
```

## Streaming

```rust
use futures::StreamExt;

let mut stream = conn.query("SELECT name FROM big_table").fetch_stream::<String>();
while let Some(name) = stream.next().await {
    process(name?);
}
```

## Transactions

`begin()` returns a `Transaction`. Reads inside it observe its own uncommitted
writes. Dropping without `commit()` rolls back.

```rust
let tx = conn.begin().await?;
tx.execute("UPDATE accounts SET balance = balance - 100 WHERE id = 1", ()).await?;
tx.execute("UPDATE accounts SET balance = balance + 100 WHERE id = 2", ()).await?;

let seen: Vec<(i64, i64)> =
    tx.fetch_all("SELECT id, balance FROM accounts", ()).await?;  // sees the updates

tx.commit().await?;   // or tx.rollback().await
```

## Connection pool

`SqurustPool` hands out bounded handles that share one engine (the engine itself
supports concurrent readers + a single writer, so this is the correct model — it
is *not* a pool of separate file handles).

```rust
use squrust_async::SqurustPool;

let pool = SqurustPool::new("app.db", 16).await?;   // max 16 concurrent handles
let conn = pool.get().await?;                        // PooledConnection: Deref<Target = SqurustConnection>
let users: Vec<(i64, String)> =
    conn.query("SELECT id, name FROM users").fetch_all().await?;
// permit returns to the pool on drop
```

## Migrations

```rust
use squrust_async::{SqurustConnection, Migration};

const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, description: "create users",
        sql: "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT)" },
    Migration { version: 2, description: "create posts",
        sql: "CREATE TABLE posts(id INTEGER PRIMARY KEY, user_id INTEGER, body TEXT)" },
];

conn.migrate(MIGRATIONS).await?;   // applies unapplied versions in order, idempotently
```

`migrate` creates a `_squrust_migrations` bookkeeping table and applies only
versions it hasn't seen. The [`migrate!`](Derive-Macros#migrate) macro can build
the `&[Migration]` from a directory of `.sql` files at compile time.

## Errors

Everything returns `squrust_async::Result<T>` = `Result<T, SqurustError>`, which
wraps the lower-level `SqlError` and `StorageError` and implements
`std::error::Error`.

See also: [[Sync API]] (no-async wrapper), [[Derive Macros]] (typed rows),
[[Architecture]] (how this maps onto the engine).
