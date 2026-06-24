# SQLite Compatibility

Squrust aims to be interchangeable with SQLite at three levels: the **on-disk
file**, the **C ABI**, and the **SQL dialect**. This page describes how far each
goes — and where it stops.

## 1. On-disk file format ✅

Squrust reads and writes **real SQLite database files**. Verified
bidirectionally against the stock `sqlite3` binary (including
`PRAGMA integrity_check → ok` on multi-level trees with overflow pages).

```console
$ sq data.db "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, age INTEGER);
              INSERT INTO users(name, age) VALUES('alice',30),('bob',25)"
$ sqlite3 data.db ".dump"
CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER);
INSERT INTO users VALUES(1,'alice',30);
INSERT INTO users VALUES(2,'bob',25);
$ sqlite3 data.db "PRAGMA integrity_check"     # -> ok
```

What's implemented (see [the SQLite file-format spec](https://www.sqlite.org/fileformat.html)):

- The **100-byte header**: magic string, page size, file-format versions,
  payload fractions, change counter, in-header db size, text encoding (UTF-8),
  and the version-valid-for field kept consistent with the change counter.
- **Table b-tree pages**: leaf (`0x0d`) and interior (`0x05`), with the
  cell-pointer array and cell content area. Page 1 carries the b-tree header at
  offset 100, after the file header.
- **Varints** — SQLite's big-endian base-128 encoding.
- **Record format** — the serial-type header + value bodies (see below).
- **Overflow pages** for large records (the SQLite spillover thresholds).
- **`sqlite_master`** rooted at page 1, with `(type, name, tbl_name, rootpage,
  sql)` rows. The `INTEGER PRIMARY KEY` (rowid alias) column is stored as NULL
  and recovered from the b-tree key on read, exactly like SQLite.

### Record format

A row's non-rowid columns are stored as a SQLite record: a header of
`varint(header_len)` followed by one **serial type** varint per column, then the
value bodies. Serial types: `0`=NULL, `1`–`6`=big-endian signed int (1/2/3/4/6/8
bytes), `7`=IEEE f64, `8`=integer 0, `9`=integer 1, even `N≥12`=BLOB of
`(N-12)/2` bytes, odd `N≥13`=TEXT of `(N-13)/2` bytes.

### Caveats

- **Page size is fixed at 4096** — Squrust can't open a `.db` written with a
  different page size.
- The **WAL is Squrust's own format** (`<db>-squrust-wal`), so crash recovery
  isn't interchangeable; the **checkpointed main file** is what's portable.
  Squrust marks the main file as journal-mode so `sqlite3` opens it directly.
- The freelist isn't persisted (freed pages may leak space), though
  `integrity_check` still passes.
- **Indexes are not built** as on-disk b-trees yet, so they aren't written to
  `sqlite_master` (the file stays valid for `sqlite3`).

## 2. C ABI / `LD_PRELOAD` ✅

`libsqurust.so` is a drop-in for `libsqlite3` for the supported surface,
verified with Python's stdlib `sqlite3`. Full details, symbol list, and what's
stubbed: **[[C ABI and LD_PRELOAD]]**.

## 3. SQL dialect

A parity battery matches stock `sqlite3` **exactly** for the supported features.

### Supported

- **DQL:** `SELECT` with `WHERE`, `ORDER BY` (incl. by ordinal and by output
  alias), `LIMIT`/`OFFSET`, `GROUP BY` + `COUNT`/`SUM`/`AVG`/`MIN`/`MAX`
  (with `DISTINCT`), expressions, `CASE` (simple & searched), `CAST`.
- **DML:** `INSERT` (multi-row `VALUES`, `INSERT OR REPLACE`), `UPDATE`,
  `DELETE`.
- **DDL:** `CREATE TABLE`, `CREATE INDEX` (recorded, not yet used), `DROP TABLE`.
- **Joins:** inner and left-outer, **two tables**, via nested-loop.
- **Transactions:** `BEGIN`/`COMMIT`/`ROLLBACK` (through the async and C APIs).
- **Type affinity:** SQLite's rules — BLOB/NONE does no conversion, INTEGER
  keeps fractional reals as real, TEXT stringifies numbers.
- **Literals:** integers, reals, strings, `NULL`, booleans, `x'..'` blobs,
  `?`/`$n` parameters.
- **Operators:** arithmetic (integer vs real division like SQLite), comparisons
  with 3-valued NULL logic, `AND`/`OR`/`NOT`, `||`, `LIKE`, `IN (..)`,
  `BETWEEN`, `IS [NOT] NULL`.
- **Scalar functions:** `length`, `upper`, `lower`, `abs`, `round`, `coalesce`,
  `ifnull`, `nullif`, `typeof`, `substr`/`substring`, `replace`, `trim`/`ltrim`/
  `rtrim`, `instr`, `hex`, `char`, `unicode`, `sign`, `quote`.

### Limitations

These are **not** implemented yet (a non-exhaustive list):

- **Index usage** — the planner always table-scans; `CREATE INDEX` is accepted
  but inert, and `UNIQUE`/non-PK constraints aren't enforced.
- **Joins beyond two tables**, comma joins, `RIGHT`/`FULL` joins, `USING`.
- **Subqueries**, CTEs (`WITH`), set operations (`UNION`/`INTERSECT`/`EXCEPT`).
- **`HAVING`**, `DISTINCT` on `SELECT`, window functions.
- **`ALTER TABLE`**, foreign keys, triggers, views, `AUTOINCREMENT` semantics
  (plain rowid allocation is used).
- **User-defined functions / collations**, most `PRAGMA`s (parsed as no-ops),
  date/time functions, JSON1, FTS, math extensions.
- **Float display** differs from SQLite (Rust's shortest round-trip vs SQLite's
  15-significant-digit formatting) — the **stored value is identical**, only the
  text rendering differs.
- `:memory:` is backed by a private temp file that's removed on close (behaves
  like an in-memory DB, but isn't literally RAM-only).

See [[Roadmap]] for what's prioritized next, and [[SQL Engine]] for how the
supported subset is implemented.
