# Sync API (`squrust-sync`)

A thin **blocking** wrapper over the [[Async API]] for code that doesn't want
`async`/`await`. Each call drives the async future to completion on a runtime:
a private current-thread Tokio runtime when called outside any reactor, or the
ambient runtime via `block_in_place` when called from within one.

```toml
[dependencies]
squrust-sync = { git = "https://github.com/ENC4YP7ED/squrust" }
```

No `async` appears anywhere in your code:

```rust
use squrust_sync::SyncConnection;

fn main() -> squrust_sync::Result<()> {
    let conn = SyncConnection::open_memory()?;          // or open("app.db")?

    conn.execute(
        "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, age INTEGER)",
        (),
    )?;
    conn.execute("INSERT INTO users(name, age) VALUES (?, ?)", ("alice", 30))?;
    conn.execute("INSERT INTO users(name, age) VALUES (?, ?)", ("bob", 25))?;

    // Typed rows, same FromRow machinery as the async API.
    let rows: Vec<(i64, String, i64)> =
        conn.query("SELECT id, name, age FROM users ORDER BY age").fetch_all()?;
    assert_eq!(rows[0].1, "bob");

    let count: i64 = conn
        .query("SELECT COUNT(*) FROM users WHERE age > ?")
        .bind(26)
        .fetch_one()?;
    assert_eq!(count, 1);

    Ok(())
}
```

## Surface

Mirrors the async API with blocking signatures:

- `SyncConnection` — `open`, `open_memory`, `execute`, `query`, `begin`,
  `migrate`, `checkpoint`, `last_insert_rowid`.
- `SyncQuery` — `bind`, `fetch_all`, `fetch_one`, `fetch_optional`, `execute`.
- `SyncTransaction` — `execute`, `fetch_all`, `commit`, `rollback`.
- `SyncPool` / `SyncPooledConnection` — `new`, `open_memory`, `get`.

```rust
let tx = conn.begin()?;
tx.execute("INSERT INTO t(id, v) VALUES (1, 'a')", ())?;
let inside: Vec<i64> = tx.fetch_all("SELECT id FROM t", ())?;
tx.commit()?;
```

> Calling the sync API from *inside* a multi-thread Tokio runtime uses
> `block_in_place`; from inside a single-thread runtime it would deadlock — use
> the [[Async API]] there instead.

See also: [[Async API]], [[Derive Macros]].
