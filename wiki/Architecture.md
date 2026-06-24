# Architecture

Squrust is a Cargo workspace of nine crates arranged in a **strict, one-way
dependency stack**. Nothing imports a crate above it.

```
            squrust-cli      squrust-wasm        (user-facing front ends)
                 │                │
                 ▼                ▼
            squrust-async  ◄── squrust-sync       (async API + blocking wrapper)
                 │
        ┌────────┼─────────┐
        ▼        ▼         ▼
  squrust-serde  │   squrust-macros               (traits + proc macros)
        │        │
        ▼        ▼
            squrust-sql                            (SQL: parse/plan/optimize/execute)
                 │
                 ▼
            squrust-core                           (storage engine)

  squrust-ffi ──► squrust-sql, squrust-core        (C ABI; no async/sync deps)
```

| Crate | Responsibility | Wiki |
|-------|----------------|------|
| `squrust-core` | Pages, LRU cache, WAL, MVCC, B-tree, `StorageEngine`/`ReadTx`/`WriteTx` | [[Storage Engine]] |
| `squrust-sql` | `&str` → `Statement` → `LogicalPlan` → executor tree → `Row`s; the catalog | [[SQL Engine]] |
| `squrust-serde` | `FromRow` / `ToParams` traits + blanket impls | [[Derive Macros]] |
| `squrust-async` | Connections, `Query`, `RowStream`, `Transaction`, pool, migrations | [[Async API]] |
| `squrust-sync` | Blocking wrapper that drives `squrust-async` on a runtime | [[Sync API]] |
| `squrust-ffi` | `libsqlite3`-compatible C ABI (`libsqurust.so`/`.a`) | [[C ABI and LD_PRELOAD]] |
| `squrust-macros` | `derive(FromRow/ToParams)`, `sql!`, `migrate!` | [[Derive Macros]] |
| `squrust-cli` | The `sq` shell | [[CLI]] |
| `squrust-wasm` | Browser bindings (`SqurustDb`) | — |

## Design invariants

These are enforced by the build, tests, and `clippy -D warnings`:

1. **Dependency direction is one-way and downward.** `cli`/`wasm` → `async` →
   `sql` → `core`. `squrust-ffi` depends only on `sql` and `core`.
2. **`#![forbid(unsafe_code)]` everywhere except `squrust-ffi`**, which is the C
   ABI boundary. The storage engine uses positioned `pread`/`pwrite` rather than
   `mmap`, so even `squrust-core` is `unsafe`-free.
3. **The core engine is synchronous.** The async boundary lives in
   `squrust-async`. This is the correct altitude for a "drop-in async" engine
   and keeps the recursive B-tree code free of boxed futures. Executors are
   `async fn`s that wrap synchronous core calls; the async surface composes
   cleanly upward.
4. **MVCC is WAL-versioned snapshot isolation** — the same discipline SQLite
   uses in WAL mode.
5. **The main `.db` file is byte-level SQLite.**

## How a query flows

Take `conn.query("SELECT name FROM users WHERE age > ?").bind(30).fetch_all::<String>()`:

1. **`squrust-async`** turns the builder into a call to `SqlEngine::query(sql, params)`.
2. **`squrust-sql`** parses the SQL (via [`sqlparser`](https://crates.io/crates/sqlparser)),
   resolves names against the in-memory **catalog**, builds a `LogicalPlan`
   (`Scan → Filter → Project`), runs a small **optimizer** (constant folding),
   and constructs a **volcano executor tree**.
3. The `TableScan` executor opens a **`ReadTx`** on **`squrust-core`** at a
   pinned snapshot version and walks the table **B-tree**, decoding each
   [record](SQLite-Compatibility#record-format) into a `Row`.
4. `Filter`/`Project` evaluate expressions per row; rows stream up through the
   tree.
5. **`squrust-async`** wraps each `Row` in a `SqurustRow` and maps it to your
   type via [`FromRow`](Derive-Macros).

Writes (`INSERT`/`UPDATE`/`DELETE`/DDL) take a **`WriteTx`** instead, buffer
their page changes, and flush them to the WAL atomically at commit. See
[[Storage Engine]] for the transaction and MVCC details, and [[SQL Engine]] for
the planner/executor.

## Concurrency model

- **Many readers, one writer.** Readers each capture a snapshot version; the
  single writer is serialised by a write gate. This mirrors SQLite's WAL mode.
- A write transaction buffers all its dirty pages locally and appends them to
  the WAL in one atomic, fsync'd batch at commit, so readers never observe a
  partial transaction.
- A `checkpoint()` folds the WAL back into the main file (and is what makes the
  on-disk file a complete, stock-`sqlite3`-readable database).
