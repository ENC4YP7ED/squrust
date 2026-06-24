# Building & Testing

## Requirements

- **Rust 1.80+** (built and tested on stable). Linux/Unix (the storage engine
  uses positioned `pread`/`pwrite`).
- Optional, for some interop tests: the `sqlite3` CLI and `python3` — tests that
  use them **skip gracefully** when they're absent.

## Build everything

```console
$ cargo build --workspace
$ cargo build --workspace --release
```

## Build a specific artifact

```console
$ cargo build --release -p squrust-ffi   # libsqurust.{so,a} in target/release/
$ cargo build --release -p squrust-cli   # the `sq` binary
```

## Test

```console
$ cargo test --workspace          # ~80 tests across all crates
$ cargo clippy --workspace --all-targets -- -D warnings
```

Notable test files:

| File | What it covers |
|------|----------------|
| `squrust-core/tests/engine.rs` | 10k-row reopen, crash recovery, partial-write truncation |
| `squrust-sql/tests/sql.rs` | CRUD, joins, aggregates, affinity, `CAST`, `CASE`, functions |
| `squrust-async/tests/api.rs` | typed fetch, streaming, transactions, migrations, concurrent pool |
| `squrust-sync/tests/sync.rs` | blocking CRUD/txn/pool (no `async` in the test) |
| `squrust-ffi/tests/c_abi.c` | a C client linked against `libsqurust` |
| `squrust-ffi/tests/ffi.rs` | the C ABI exercised from Rust |
| `squrust-ffi/tests/ld_preload.rs` | Python's `sqlite3` under `LD_PRELOAD` (skips if absent) |
| `squrust-cli/tests/interop.rs` | bidirectional `.db` interchange with the `sqlite3` binary |
| `squrust-macros/tests/macros.rs` | `derive`, `sql!`, `migrate!` end-to-end |

## Try the interop yourself

```console
$ cargo build --release -p squrust-cli -p squrust-ffi

# squrust writes, sqlite3 reads:
$ ./target/release/sq data.db "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t(v) VALUES('hi')"
$ sqlite3 data.db "SELECT * FROM t"          # -> 1|hi

# LD_PRELOAD a Python script:
$ LD_PRELOAD=$PWD/target/release/libsqurust.so python3 -c \
  "import sqlite3; d=sqlite3.connect(':memory:'); d.execute('CREATE TABLE t(x)'); \
   d.execute('INSERT INTO t VALUES(1)'); print(d.execute('SELECT * FROM t').fetchall())"
# -> [(1,)]
```

## Wasm (experimental)

`squrust-wasm` exposes a `SqurustDb` class via `wasm-bindgen`. The portable core
is host-testable (`cargo test -p squrust-wasm`), but a real `wasm32` build needs
an in-memory storage backend in `squrust-core` (its file I/O is Unix-specific
today) — see [[Roadmap]].

See also: [[Architecture]], [[SQLite Compatibility]].
