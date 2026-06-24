# Roadmap

Squrust is a working prototype. These are the things that would most increase
its usefulness and compatibility, roughly in priority order. Contributions
welcome — open an [issue](https://github.com/ENC4YP7ED/squrust/issues) to
discuss.

## Query engine

- **Index b-trees** — build real index b-trees (page types `0x0a`/`0x02`),
  persist them in `sqlite_master`, and use them in the planner for point lookups
  and range scans. Today the executor always table-scans.
- **Constraint enforcement** — extend `UNIQUE` to `UPDATE`/`DELETE` (enforced on
  `INSERT` today, by scan) and to non-rowid `PRIMARY KEY`, then back it with
  index b-trees instead of a full scan.
- **Joins** — 3+ tables, comma joins, `RIGHT`/`FULL` outer, hash joins.
- **Subqueries / CTEs / set operations**.
- **`ALTER TABLE`** beyond `ADD COLUMN` (rename table/column, drop column),
  foreign keys, triggers, views.

## SQL surface

- More built-ins: `printf`, `glob`, math, JSON1.
- **User-defined functions/collations** through the C API (`create_function`,
  `create_collation`) — currently accepted but inert.
- Match SQLite's **float text formatting** (15 significant digits) so rendered
  output is byte-identical (stored values already match).

## Storage

- **Configurable page size** so Squrust can open `.db` files that don't use the
  4096-byte default.
- **Persisted freelist** so freed pages are reused and `.db` files don't grow
  monotonically.
- Optional **SQLite-format WAL** for crash-recovery interchange (today the WAL
  is Squrust's own format; only the checkpointed main file is portable).
- True in-memory `:memory:` backend (currently a private temp file).

## C ABI

- Flesh out the stubbed features: `blob_*` I/O, online `backup_*`,
  `serialize`/`deserialize`, extension loading, the `value_*`/`result_*` path
  for user functions.
- Broaden `PRAGMA` handling beyond no-ops.

## Wasm / browser

- An in-memory storage backend in `squrust-core` so the whole stack compiles to
  `wasm32`, then an **OPFS**-backed backend for persistence. The `SqurustDb`
  `wasm-bindgen` surface already exists.

## Tooling

- CI (build + test + clippy on every push), benchmarks vs. SQLite, fuzz targets
  for the SQL parser and page decoder.

See [[SQLite Compatibility]] for the current limitations these items address.
