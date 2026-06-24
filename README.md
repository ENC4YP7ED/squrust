# Squrust 🐿️🌰

**A drop-in, async, SQLite-compatible database engine written from scratch in Rust.**

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust 1.80+](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
[![SQLite file-format compatible](https://img.shields.io/badge/sqlite-file--format%20compatible-success.svg)](https://github.com/ENC4YP7ED/squrust/wiki/SQLite-Compatibility)
[![unsafe: FFI only](https://img.shields.io/badge/unsafe-FFI%20boundary%20only-informational.svg)](#design-invariants)

Squrust is a storage engine, SQL engine, and family of language bindings that
together produce **two interchangeable things**:

1. **`libsqurust.so` / `libsqurust.a`** — a C ABI compatible with `libsqlite3`.
   `LD_PRELOAD` it and existing programs (including Python's `sqlite3`) talk to
   Squrust with **zero code changes**.
2. **`squrust-async`** — an idiomatic async Rust crate with a query builder,
   typed row mapping, streaming results, connection pooling, and `serde`-style
   derives.

It reads and writes **real SQLite database files** — `sqlite3 mydata.db .dump`
works on a file Squrust wrote, and Squrust opens files `sqlite3` wrote.

> ⚠️ **Status: an ambitious, working prototype — not production-ready.** The
> common path (CRUD, joins, aggregates, transactions, file interchange,
> `LD_PRELOAD`) works and is tested, but there are real
> [limitations](https://github.com/ENC4YP7ED/squrust/wiki/SQLite-Compatibility#limitations)
> (no index b-trees yet, single/two-table joins, etc.). See the
> [Roadmap](https://github.com/ENC4YP7ED/squrust/wiki/Roadmap).

---

## Table of contents

- [Why](#why)
- [Quick start](#quick-start)
  - [Async Rust](#async-rust)
  - [Typed rows with derives](#typed-rows-with-derives)
  - [Sync Rust](#sync-rust)
  - [The `sq` CLI](#the-sq-cli)
  - [Drop in for libsqlite3 (LD_PRELOAD)](#drop-in-for-libsqlite3-ld_preload)
  - [Interop with stock `sqlite3`](#interop-with-stock-sqlite3)
- [Workspace layout](#workspace-layout)
- [Design invariants](#design-invariants)
- [Building & testing](#building--testing)
- [Documentation (wiki)](#documentation-wiki)
- [License](#license)

---

## Why

SQLite is everywhere, but its C core is synchronous and single-threaded by
design. Squrust is an experiment in answering: *what if the same on-disk format
and the same C ABI were backed by a Rust engine with MVCC snapshot isolation and
a first-class async API?* You get to keep your `.db` files and your `libsqlite3`
call sites, while new Rust code gets `async`/`await`, typed queries, and a
connection pool.

Highlights:

- 🔁 **Bidirectional file-format compatibility** — real SQLite b-tree pages,
  record format, varints, overflow pages, `sqlite_master` on page 1.
  ([details](https://github.com/ENC4YP7ED/squrust/wiki/SQLite-Compatibility))
- 🧵 **MVCC snapshot isolation** via a write-ahead log — concurrent readers never
  block the single writer.
  ([details](https://github.com/ENC4YP7ED/squrust/wiki/Storage-Engine))
- ⚡ **Async-first API** with a query builder, `RowStream`, transactions, and a
  pool. ([details](https://github.com/ENC4YP7ED/squrust/wiki/Async-API))
- 🔌 **`LD_PRELOAD` drop-in** for `libsqlite3`, verified against Python's stdlib
  `sqlite3`. ([details](https://github.com/ENC4YP7ED/squrust/wiki/C-ABI-and-LD_PRELOAD))
- 🧱 **No `unsafe`** anywhere except the C ABI boundary crate.
- 🖥️ **`sq`** — a `sqlite3`-style CLI with table/CSV/JSON/line output.
  ([details](https://github.com/ENC4YP7ED/squrust/wiki/CLI))
- 🧩 **Derive macros** — `#[derive(FromRow, ToParams)]`, compile-time-checked
  `sql!`, and `migrate!`.
  ([details](https://github.com/ENC4YP7ED/squrust/wiki/Derive-Macros))

---

## Quick start

The crates aren't published to crates.io yet — depend on them via git:

```toml
[dependencies]
squrust-async  = { git = "https://github.com/ENC4YP7ED/squrust" }
squrust-macros = { git = "https://github.com/ENC4YP7ED/squrust" } # optional: derives
tokio          = { version = "1", features = ["full"] }
```

### Async Rust

```rust
use squrust_async::SqurustConnection;

#[tokio::main]
async fn main() -> squrust_async::Result<()> {
    let conn = SqurustConnection::open("app.db").await?;  // or open_memory()

    conn.execute(
        "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, age INTEGER)",
        (),
    ).await?;

    // Bound parameters via tuples, slices, or .bind() chains.
    conn.execute("INSERT INTO users(name, age) VALUES (?, ?)", ("ada", 36)).await?;

    // Typed, by-position or by-name; tuples and scalars implement FromRow.
    let rows: Vec<(i64, String, i64)> =
        conn.query("SELECT id, name, age FROM users WHERE age > ? ORDER BY age")
            .bind(30)
            .fetch_all()
            .await?;

    let count: i64 = conn.query("SELECT COUNT(*) FROM users").fetch_one().await?;
    println!("{count} users: {rows:?}");

    // Transactions (commit / rollback / implicit-rollback-on-drop).
    let tx = conn.begin().await?;
    tx.execute("INSERT INTO users(name, age) VALUES ('grace', 45)", ()).await?;
    tx.commit().await?;

    Ok(())
}
```

Streaming with `futures::StreamExt`:

```rust
use futures::StreamExt;

let mut stream = conn.query("SELECT name FROM users").fetch_stream::<String>();
while let Some(name) = stream.next().await {
    println!("{}", name?);
}
```

See the [Async API guide](https://github.com/ENC4YP7ED/squrust/wiki/Async-API)
for the pool, migrations, and the full `Query` builder.

### Typed rows with derives

```rust
use squrust_async::{SqurustConnection, ToParams};   // ToParams trait
use squrust_macros::{FromRow, ToParams};            // derive macros (separate namespace)

#[derive(Debug, FromRow, ToParams)]
struct User {
    id: i64,
    name: String,
    email: Option<String>,   // NULL maps to None
}

# async fn demo(conn: &SqurustConnection) -> squrust_async::Result<()> {
let alice = User { id: 1, name: "alice".into(), email: Some("a@example.com".into()) };
conn.execute("INSERT INTO users(id, name, email) VALUES (?, ?, ?)", alice.to_params()).await?;

let users: Vec<User> = conn.query("SELECT id, name, email FROM users").fetch_all().await?;
# Ok(()) }
```

The [`sql!`](https://github.com/ENC4YP7ED/squrust/wiki/Derive-Macros#sql) macro
validates table/column names against a schema file *at compile time*, and
[`migrate!`](https://github.com/ENC4YP7ED/squrust/wiki/Derive-Macros#migrate)
embeds a directory of `.sql` files as ordered migrations.

### Sync Rust

No `async` in your code? Use `squrust-sync` (a blocking wrapper):

```rust
use squrust_sync::SyncConnection;

fn main() -> squrust_sync::Result<()> {
    let conn = SyncConnection::open_memory()?;
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)", ())?;
    conn.execute("INSERT INTO t(v) VALUES (?)", ("hello",))?;
    let vals: Vec<String> = conn.query("SELECT v FROM t").fetch_all()?;
    assert_eq!(vals, vec!["hello".to_string()]);
    Ok(())
}
```

### The `sq` CLI

```console
$ cargo install --git https://github.com/ENC4YP7ED/squrust squrust-cli   # installs `sq`

$ sq mydata.db "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT);
                INSERT INTO t(name) VALUES('alice'),('bob');
                SELECT * FROM t"
┌────┬───────┐
│ id ┆ name  │
╞════╪═══════╡
│ 1  ┆ alice │
├╌╌╌╌┼╌╌╌╌╌╌╌┤
│ 2  ┆ bob   │
└────┴───────┘

$ echo "SELECT 1 + 1 AS answer" | sq :memory: --mode json
[ { "answer": 2 } ]

$ sq mydata.db      # interactive REPL: .tables, .schema, .mode, .import, .help
```

More in the [CLI guide](https://github.com/ENC4YP7ED/squrust/wiki/CLI).

### Drop in for libsqlite3 (LD_PRELOAD)

```console
$ cargo build --release -p squrust-ffi      # builds target/release/libsqurust.so

$ LD_PRELOAD=$PWD/target/release/libsqurust.so python3 - <<'PY'
import sqlite3
con = sqlite3.connect(":memory:")
con.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")
con.executemany("INSERT INTO t(name, age) VALUES (?, ?)",
                [("alice", 30), ("bob", 25)])
con.commit()
print(con.execute("SELECT name, age FROM t ORDER BY age").fetchall())
# -> [('bob', 25), ('alice', 30)]   ← served by Squrust, not libsqlite3
PY
```

`sqlite3.sqlite_version` reports `3.45.0` under preload — that's Squrust
answering. Full details (symbol coverage, transactions, what's stubbed) in the
[C ABI & LD_PRELOAD guide](https://github.com/ENC4YP7ED/squrust/wiki/C-ABI-and-LD_PRELOAD).

### Interop with stock `sqlite3`

```console
$ sq data.db "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, age INTEGER);
              INSERT INTO users(name, age) VALUES('alice', 30), ('bob', 25)"

$ sqlite3 data.db ".dump"        # stock sqlite3 reads Squrust's file
CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER);
INSERT INTO users VALUES(1,'alice',30);
INSERT INTO users VALUES(2,'bob',25);

$ sqlite3 data.db "PRAGMA integrity_check"   # -> ok
```

…and the reverse (`sq` opening a file `sqlite3` created) works too. See
[SQLite Compatibility](https://github.com/ENC4YP7ED/squrust/wiki/SQLite-Compatibility).

---

## Workspace layout

Squrust is a Cargo workspace of nine crates with a strict one-way dependency
direction (`cli`/`wasm` → `async` → `sql` → `core`):

| Crate | What it is |
|-------|-----------|
| [`squrust-core`](https://github.com/ENC4YP7ED/squrust/wiki/Storage-Engine) | Storage engine: pages, LRU cache, WAL, MVCC, B-tree, transactions |
| [`squrust-sql`](https://github.com/ENC4YP7ED/squrust/wiki/SQL-Engine) | SQL parser, planner, optimizer, volcano executor, schema catalog |
| `squrust-serde` | `FromRow` / `ToParams` traits + blanket impls |
| [`squrust-async`](https://github.com/ENC4YP7ED/squrust/wiki/Async-API) | The primary async API: connections, query builder, streams, pool, migrations |
| [`squrust-sync`](https://github.com/ENC4YP7ED/squrust/wiki/Sync-API) | Blocking wrapper over `squrust-async` |
| [`squrust-ffi`](https://github.com/ENC4YP7ED/squrust/wiki/C-ABI-and-LD_PRELOAD) | C ABI (`libsqurust.so`/`.a`) — the `libsqlite3` drop-in |
| [`squrust-macros`](https://github.com/ENC4YP7ED/squrust/wiki/Derive-Macros) | Proc macros: `derive(FromRow/ToParams)`, `sql!`, `migrate!` |
| [`squrust-cli`](https://github.com/ENC4YP7ED/squrust/wiki/CLI) | The `sq` command-line shell |
| `squrust-wasm` | Browser/wasm bindings (`SqurustDb`) — portable core, OPFS WIP |

Full picture: [Architecture](https://github.com/ENC4YP7ED/squrust/wiki/Architecture).

---

## Design invariants

These hold across the codebase (and are enforced by tests + `clippy`):

- **Dependency direction is one-way and downward.** No crate imports one above
  it in the stack.
- **`#![forbid(unsafe_code)]` everywhere except `squrust-ffi`**, which is the C
  ABI boundary. The storage engine uses positioned `pread`/`pwrite`, not `mmap`,
  so even `squrust-core` is `unsafe`-free.
- **The core engine is synchronous;** the async boundary lives in
  `squrust-async`. This is the right altitude for a "drop-in async" engine and
  avoids boxed recursive futures in the b-tree.
- **MVCC = WAL-versioned snapshot isolation,** the same discipline SQLite uses in
  WAL mode: a reader pinned at version *V* only sees commits `≤ V`.
- **The main `.db` file is byte-level SQLite.** (Squrust's WAL sidecar uses its
  own format and is named `<db>-squrust-wal` so it never collides with SQLite's
  `-wal`; a checkpointed file is plain journal-mode SQLite.)

---

## Building & testing

Requires Rust **1.80+** (built/tested on stable).

```console
$ cargo build --workspace
$ cargo test  --workspace          # ~80 tests across all crates
$ cargo clippy --workspace --all-targets -- -D warnings

# C ABI drop-in library:
$ cargo build --release -p squrust-ffi      # -> target/release/libsqurust.{so,a}
```

Some tests shell out to the real `sqlite3` binary (file interop) and `python3`
(LD_PRELOAD); they **skip gracefully** when those aren't installed. See
[Building & Testing](https://github.com/ENC4YP7ED/squrust/wiki/Building-and-Testing).

---

## Documentation (wiki)

| Page | Contents |
|------|----------|
| [Home](https://github.com/ENC4YP7ED/squrust/wiki) | Overview & navigation |
| [Architecture](https://github.com/ENC4YP7ED/squrust/wiki/Architecture) | How the layers fit together, request lifecycle |
| [Storage Engine](https://github.com/ENC4YP7ED/squrust/wiki/Storage-Engine) | Pages, B-tree, WAL, MVCC, transactions |
| [SQL Engine](https://github.com/ENC4YP7ED/squrust/wiki/SQL-Engine) | Parser → planner → optimizer → executor; the catalog |
| [Async API](https://github.com/ENC4YP7ED/squrust/wiki/Async-API) | Connections, `Query`, streams, transactions, pool, migrations |
| [Sync API](https://github.com/ENC4YP7ED/squrust/wiki/Sync-API) | The blocking wrapper |
| [Derive Macros](https://github.com/ENC4YP7ED/squrust/wiki/Derive-Macros) | `FromRow`, `ToParams`, `sql!`, `migrate!` |
| [C ABI & LD_PRELOAD](https://github.com/ENC4YP7ED/squrust/wiki/C-ABI-and-LD_PRELOAD) | The `libsqlite3` drop-in, symbol coverage, Python |
| [CLI](https://github.com/ENC4YP7ED/squrust/wiki/CLI) | The `sq` shell, output modes, dot-commands |
| [SQLite Compatibility](https://github.com/ENC4YP7ED/squrust/wiki/SQLite-Compatibility) | File format, dialect parity, **limitations** |
| [Building & Testing](https://github.com/ENC4YP7ED/squrust/wiki/Building-and-Testing) | Build matrix, running the suites |
| [Roadmap](https://github.com/ENC4YP7ED/squrust/wiki/Roadmap) | What's next |

> The source for every wiki page also lives in [`wiki/`](wiki/). GitHub only
> creates a repo's wiki after its first page is made in the browser (there's no
> API for it), so to populate the Wiki tab: create the first page once at
> [`/wiki/_new`](https://github.com/ENC4YP7ED/squrust/wiki/_new), then run
> [`./scripts/publish-wiki.sh`](scripts/publish-wiki.sh).

---

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion shall be dual-licensed as above, without
any additional terms or conditions.

---

*Squrust is an independent project and is not affiliated with or endorsed by
SQLite or its authors. "SQLite" is a trademark of Hipp, Wyrick & Company, Inc.*
