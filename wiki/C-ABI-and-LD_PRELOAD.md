# C ABI & LD_PRELOAD (`squrust-ffi`)

`squrust-ffi` builds a C library whose ABI matches `libsqlite3`, so programs
that link (or `dlopen`) `libsqlite3` can talk to Squrust unchanged.

- Crate type: `cdylib` + `staticlib` + `rlib`. The library is named **`squrust`**,
  so the artifacts are `libsqurust.so` and `libsqurust.a`.
- Depends only on `squrust-sql` and `squrust-core` (no async/sync crates).
- Each connection drives the async `SqlEngine` via a tiny synchronous
  future-poller — **no Tokio runtime in the FFI** (running a reactor across
  CPython's GIL-release boundary corrupted results, so it was removed).

## Build

```console
$ cargo build --release -p squrust-ffi
# -> target/release/libsqurust.so  and  libsqurust.a
```

A C header, `squrust-ffi/squrust.h`, declares the supported surface and is a
drop-in for the subset of `sqlite3.h` it covers.

## Use it from C

```c
#include "squrust.h"   // or your existing <sqlite3.h>

sqlite3 *db;
sqlite3_open(":memory:", &db);
sqlite3_exec(db, "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)", 0, 0, 0);

sqlite3_stmt *st;
sqlite3_prepare_v2(db, "INSERT INTO t(name) VALUES (?)", -1, &st, 0);
sqlite3_bind_text(st, 1, "alice", -1, NULL);
sqlite3_step(st);
sqlite3_finalize(st);

sqlite3_close(db);
```

```console
# link directly against Squrust:
$ cc app.c -L target/release -lsqurust -lpthread -lm -ldl -o app

# or LD_PRELOAD over a binary built against the system libsqlite3:
$ cc app.c -lsqlite3 -o app
$ LD_PRELOAD=$PWD/target/release/libsqurust.so ./app
```

## Use it from Python (LD_PRELOAD)

Python's stdlib `sqlite3` works against Squrust with no code changes:

```console
$ LD_PRELOAD=$PWD/target/release/libsqurust.so python3 - <<'PY'
import sqlite3
print("engine:", sqlite3.sqlite_version)      # -> 3.45.0  (Squrust answering)

con = sqlite3.connect(":memory:")
con.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")
con.executemany("INSERT INTO t(name, age) VALUES (?, ?)",
                [("alice", 30), ("bob", 25), ("carol", 41)])
con.commit()

print(con.execute("SELECT name, age FROM t WHERE age > 26 ORDER BY age").fetchall())
# -> [('alice', 30), ('carol', 41)]

con.row_factory = sqlite3.Row
r = con.execute("SELECT name FROM t WHERE id = 1").fetchone()
print(r["name"])                               # -> alice

# real transactions:
con.execute("UPDATE t SET age = 0 WHERE name = 'alice'")
con.rollback()
print(con.execute("SELECT age FROM t WHERE name='alice'").fetchone()[0])  # -> 30
PY
```

## How LD_PRELOAD works here

Under `LD_PRELOAD`, the dynamic linker resolves `sqlite3_*` symbols to Squrust
first. **Every** symbol the host program references must therefore be defined by
Squrust — otherwise a missing one falls through to the real `libsqlite3`, which
then operates on a Squrust handle and crashes. Squrust exports **100+**
`sqlite3_*` symbols:

- **Implemented:** `open`/`open_v2`/`close`/`close_v2`, `exec`,
  `prepare_v2`/`finalize`/`sql`/`stmt_readonly`/`stmt_busy`,
  `step`/`reset`/`clear_bindings`, all `bind_*`, all `column_*`,
  `errcode`/`errmsg`/`errstr`, `changes`/`changes64`/`last_insert_rowid`,
  `get_autocommit`, `libversion*`, `complete`, `db_handle`, `free`/`malloc`, …
- **Transactions:** `BEGIN`/`COMMIT`/`ROLLBACK` issued through the C API hold a
  real write transaction on the connection; `get_autocommit` reflects it. A key
  detail for CPython: `sqlite3_stmt_busy` is implemented correctly — without it,
  CPython discards every result set.
- **Safe stubs (accepted but inert):** `create_function*`, `create_collation*`,
  `db_config`, `set_authorizer`, `trace_v2`, `progress_handler`, `blob_*`,
  `backup_*`, `serialize`/`deserialize`, extension loading. These let hosts load
  and configure, but those *features* aren't functional yet (see [[Roadmap]]).

## Statement lifecycle (implementation notes)

- `prepare_v2` parses and validates the SQL (syntax errors surface here);
  column metadata is available immediately via `describe` (so
  `sqlite3_column_count`/`_name` work before `step`).
- On the first `step`, the statement executes and **materialises** its result
  rows; subsequent `step`s walk the cached rows. DML returns `SQLITE_DONE`.
- `:memory:` maps to a private temp-file-backed engine (behaves like SQLite's
  in-memory DB; see [[SQLite Compatibility]]).

## Verified

- `squrust-ffi/tests/c_abi.c` — a C client linked against `libsqurust`.
- `squrust-ffi/tests/ffi.rs` — exercises the symbols from Rust.
- `squrust-ffi/tests/ld_preload.rs` — runs Python under `LD_PRELOAD` (skips if
  `python3`/the `.so` aren't present).

See also: [[SQLite Compatibility]], [[Building and Testing]].
