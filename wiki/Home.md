# Squrust Wiki 🐿️🌰

**Squrust** is a drop-in, async, SQLite-compatible database engine written from
scratch in Rust. It produces two interchangeable artifacts:

1. **`libsqurust.so` / `libsqurust.a`** — a C ABI compatible with `libsqlite3`,
   usable as an `LD_PRELOAD` drop-in.
2. **`squrust-async`** — an idiomatic async Rust crate.

It reads and writes **real SQLite database files**, so `sqlite3 file.db .dump`
works on a file Squrust wrote, and vice-versa.

> This wiki is the developer reference. For a quick taste and install
> instructions, start with the [README](https://github.com/ENC4YP7ED/squrust#readme).

## Map of the docs

- **[[Architecture]]** — how the nine crates stack up and how a query flows
  through them.
- **[[Storage Engine]]** — `squrust-core`: pages, the LRU cache, the WAL,
  MVCC snapshot isolation, the B-tree, and transactions.
- **[[SQL Engine]]** — `squrust-sql`: parser → planner → optimizer → executor,
  the schema catalog, value/affinity model, and row encoding.
- **[[Async API]]** — `squrust-async`: connections, the `Query` builder, typed
  rows, `RowStream`, transactions, the pool, and migrations.
- **[[Sync API]]** — `squrust-sync`: the blocking wrapper.
- **[[Derive Macros]]** — `squrust-macros`: `derive(FromRow/ToParams)`,
  compile-time-checked `sql!`, and `migrate!`.
- **[[C ABI and LD_PRELOAD]]** — `squrust-ffi`: the `libsqlite3` drop-in, its
  symbol coverage, transactions across the C boundary, and the Python story.
- **[[CLI]]** — `sq`: the `sqlite3`-style shell.
- **[[SQLite Compatibility]]** — the on-disk file format, dialect parity, and an
  honest list of **limitations**.
- **[[Building and Testing]]** — how to build each artifact and run the suites.
- **[[Roadmap]]** — what's planned next.

## Project status

Squrust is an ambitious, **working prototype**. The common path is implemented
and tested:

- ✅ CRUD, `WHERE` / `ORDER BY` / `LIMIT` / `GROUP BY`, aggregates, inner & left
  joins, `CASE`, `CAST`, many scalar functions
- ✅ Real SQLite file format (read + write), verified against the `sqlite3` CLI
- ✅ MVCC snapshot isolation over a write-ahead log
- ✅ Async API + connection pool + migrations
- ✅ `LD_PRELOAD` drop-in, verified with Python's `sqlite3`
- ✅ ~80 tests; `clippy -D warnings` clean

It is **not production-ready** — see [[SQLite Compatibility]] for the limitations
(no index b-trees yet, joins limited to two tables, etc.).
