# Squrust 🐿️🌰 — Agentic Build Plan

> Drop-in async SQLite replacement in Rust.
> Two primary outputs:
> 1. `libsqurust.so` / `libsqurust.a` — C ABI identical to `libsqlite3`, LD_PRELOAD drop-in, zero application changes required
> 2. `squrust-async` — idiomatic async Rust crate with typed queries, streams, serde integration

---

## Workspace layout

```
squrust/
├── Cargo.toml              # workspace root
├── squrust-core/           # storage engine: pages, B-tree, WAL, MVCC
├── squrust-sql/            # SQL parser, planner, optimizer, executor
├── squrust-async/          # async Rust API (primary Rust interface)
├── squrust-sync/           # blocking wrapper around squrust-async
├── squrust-ffi/            # C ABI: libsqurust.so drop-in for libsqlite3
├── squrust-macros/         # proc macros: compile-time SQL, derive macros
├── squrust-serde/          # FromRow / ToParams traits
├── squrust-wasm/           # wasm-bindgen browser target
├── squrust-cli/            # sq CLI tool
├── fuzz/                   # cargo-fuzz targets
└── benches/                # criterion benchmarks vs sqlite
```

---

## Phase execution order

Build in strict dependency order. Each phase must pass `cargo test` before the next begins.

```
1. squrust-core      (no deps on other squrust crates)
2. squrust-sql       (depends on: core)
3. squrust-serde     (depends on: sql)
4. squrust-async     (depends on: core, sql, serde)
5. squrust-sync      (depends on: async)
6. squrust-ffi       (depends on: core, sql — spawns its own tokio runtime)
7. squrust-macros    (depends on: serde; proc-macro crate)
8. squrust-wasm      (depends on: sql; alternate I/O backend)
9. squrust-cli       (depends on: async)
```

---

## Workspace Cargo.toml

```toml
[workspace]
members = [
    "squrust-core",
    "squrust-sql",
    "squrust-async",
    "squrust-sync",
    "squrust-ffi",
    "squrust-macros",
    "squrust-serde",
    "squrust-wasm",
    "squrust-cli",
]
resolver = "2"

[workspace.dependencies]
tokio          = { version = "1",    features = ["full"] }
bytes          = "1"
thiserror      = "1"
anyhow         = "1"
serde          = { version = "1",    features = ["derive"] }
serde_json     = "1"
tracing        = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
memmap2        = "0.9"
parking_lot    = "0.12"
async-trait    = "0.1"
futures        = "0.3"
async-stream   = "0.3"
sqlparser      = "0.50"
deadpool       = { version = "0.12", features = ["rt_tokio_1"] }
syn            = { version = "2",    features = ["full"] }
quote          = "1"
proc-macro2    = "1"
criterion      = { version = "0.5",  features = ["html_reports"] }
proptest       = "1"

[profile.release]
lto = true
codegen-units = 1
```

---

## Phase 1 — squrust-core

The complete storage engine. No SQL. No user API. Just pages, B-tree, WAL, and MVCC.
This crate has zero dependencies on other squrust crates.

### File structure

```
squrust-core/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── error.rs
    ├── page/
    │   ├── mod.rs          # PageId, PageType, RawPage
    │   ├── cache.rs        # LRU page cache
    │   └── format.rs       # on-disk constants, SQLite header layout
    ├── btree/
    │   ├── mod.rs          # BTree public API
    │   ├── node.rs         # BTreeNode: leaf / interior serialization
    │   ├── cursor.rs       # BTreeCursor: forward/backward iteration
    │   └── split.rs        # page split + parent pointer update
    ├── wal/
    │   ├── mod.rs          # WriteAheadLog
    │   ├── frame.rs        # WalFrame: 32-byte header + page data
    │   └── checkpoint.rs   # move WAL frames back into main file
    ├── mvcc/
    │   ├── mod.rs          # MvccManager, active transaction set
    │   ├── version.rs      # TransactionId (u64 atomic), VersionedPage
    │   └── snapshot.rs     # Snapshot: visible page versions
    └── storage/
        ├── mod.rs          # StorageEngine, ReadTx, WriteTx
        ├── file.rs         # async file I/O via tokio + mmap
        └── header.rs       # 100-byte SQLite-compatible file header
```

### Core traits and types

```rust
// page/mod.rs
pub type PageId = u32;
pub const PAGE_SIZE: usize = 4096;

pub struct RawPage {
    pub id: PageId,
    pub data: Box<[u8; PAGE_SIZE]>,
    pub dirty: bool,
}

pub enum PageType { Interior, Leaf, Overflow, FreeList }

// storage/mod.rs
pub struct StorageEngine {
    file:  Arc<DatabaseFile>,
    cache: Arc<PageCache>,
    wal:   Arc<WriteAheadLog>,
    mvcc:  Arc<MvccManager>,
}

impl StorageEngine {
    pub async fn open(path: &Path) -> Result<Self, StorageError>;
    pub async fn open_memory() -> Result<Self, StorageError>;
    pub async fn begin_read(&self)  -> Result<ReadTx,  StorageError>;
    pub async fn begin_write(&self) -> Result<WriteTx, StorageError>;
    pub async fn checkpoint(&self)  -> Result<(), StorageError>;
    pub async fn close(self)        -> Result<(), StorageError>;
}

pub struct ReadTx  { snapshot_id: TransactionId, engine: Arc<StorageEngine> }
pub struct WriteTx { id: TransactionId,           engine: Arc<StorageEngine> }

impl ReadTx {
    pub async fn get_page(&self, id: PageId) -> Result<Arc<RawPage>, StorageError>;
    pub async fn commit(self);
}

impl WriteTx {
    pub async fn get_page(&self, id: PageId)          -> Result<Arc<RawPage>, StorageError>;
    pub async fn write_page(&self, page: RawPage)     -> Result<(), StorageError>;
    pub async fn alloc_page(&self)                    -> Result<PageId, StorageError>;
    pub async fn free_page(&self, id: PageId)         -> Result<(), StorageError>;
    pub async fn commit(self)                         -> Result<(), StorageError>;
    pub async fn rollback(self);
}
```

### Key design decisions

- **Page size:** 4096 bytes. Matches SQLite default; maintains on-disk compat.
- **File header:** first 100 bytes match SQLite 3 exactly (magic string `SQLite format 3\000`, page size u16, format versions, etc.). See [SQLite file format spec §1.3](https://www.sqlite.org/fileformat.html).
- **WAL frames:** SQLite WAL format (32-byte frame header + page data). Checksum algo matches SQLite's.
- **MVCC:** timestamp-based snapshot IDs via `AtomicU64`. Active transaction set stored in `DashSet<TransactionId>`.
- **Page cache:** LRU eviction, 2000 pages default (~8 MB), configurable. Returns `Arc<RawPage>` — readers hold a clone, cache can evict once all arcs drop.
- **I/O:** `memmap2` for read path (zero-copy page reads). `tokio::fs::File` + `pwrite` for WAL frame appends.

### Tasks — Phase 1

- [ ] Create `squrust-core/Cargo.toml` (deps: tokio, bytes, thiserror, memmap2, parking_lot, tracing)
- [ ] `page/format.rs` — SQLite-compatible header constants: magic, page size offset, format versions, page count, freelist, schema version, etc.
- [ ] `page/mod.rs` — `RawPage`, `PageId`, `PageType`, page header serialization helpers
- [ ] `page/cache.rs` — LRU page cache: `Arc<RawPage>` entries, configurable capacity, async-safe eviction
- [ ] `storage/header.rs` — read/write 100-byte database header; validate magic string on open
- [ ] `storage/file.rs` — `DatabaseFile`: async open (create if missing), `read_page(id)` via mmap, `write_page()` via pwrite, `sync()`
- [ ] `wal/frame.rs` — `WalFrame`: serialize/deserialize 32-byte header (magic, page number, salt, checksum) + page data
- [ ] `wal/mod.rs` — `WriteAheadLog`: append frame, read latest version of page from WAL, exclusive write lock
- [ ] `wal/checkpoint.rs` — `checkpoint()`: copy WAL frames into main file in order, truncate WAL
- [ ] `mvcc/version.rs` — `TransactionId` (u64), `AtomicU64` global counter, `VersionedPage` (page snapshot per tx)
- [ ] `mvcc/snapshot.rs` — `Snapshot`: captures set of active transactions at begin time for visibility rules
- [ ] `mvcc/mod.rs` — `MvccManager`: begin tx, commit tx, rollback tx, is-visible check
- [ ] `btree/node.rs` — `BTreeNode`: leaf node (key-value pairs), interior node (keys + child page IDs), serialize/deserialize from `RawPage`
- [ ] `btree/cursor.rs` — `BTreeCursor`: position to key, `next()`, `prev()`, `seek(key)`
- [ ] `btree/split.rs` — page split when node is full; update parent interior node; handle root split
- [ ] `btree/mod.rs` — `BTree`: `get(key)`, `insert(key, value)`, `delete(key)`, `scan(from, to)` returning cursor
- [ ] `storage/mod.rs` — `StorageEngine`, `ReadTx`, `WriteTx` tying all of the above together
- [ ] `error.rs` — `StorageError` with `thiserror` variants for all failure modes
- [ ] Unit tests: page roundtrip, WAL frame serialize/deserialize, B-tree CRUD, MVCC snapshot isolation, LRU cache eviction
- [ ] Integration test: open file, write 10k rows, close, reopen, verify all rows readable
- [ ] Crash recovery test: write rows, fsync WAL but not main file, simulate crash, reopen and checkpoint, verify data

---

## Phase 2 — squrust-sql

SQL parser, query planner, and execution engine on top of `squrust-core` transactions.

### File structure

```
squrust-sql/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── error.rs
    ├── types.rs            # SqlType, Value enum, type coercions
    ├── row.rs              # Row, RowId, row encoding to/from page bytes
    ├── parser.rs           # thin wrapper: &str → sqlparser::Statement
    ├── schema/
    │   ├── mod.rs          # Table, Column, Index, Schema structs
    │   └── catalog.rs      # SchemaCatalog stored in sqlite_master table (page 1)
    ├── planner/
    │   ├── mod.rs          # LogicalPlan enum
    │   ├── resolver.rs     # name resolution, type checking, aliasing
    │   └── optimizer.rs    # predicate push-down, constant folding, index selection
    └── executor/
        ├── mod.rs          # Executor trait, execute()
        ├── scan.rs         # TableScan (full), IndexScan
        ├── filter.rs       # Filter: expression evaluator
        ├── projection.rs   # Projection: column selection + computed expressions
        ├── join.rs         # NestedLoopJoin (start simple)
        ├── aggregate.rs    # COUNT, SUM, AVG, MIN, MAX, GROUP BY
        ├── sort.rs         # ORDER BY (in-memory sort for now)
        ├── limit.rs        # LIMIT / OFFSET
        ├── insert.rs       # INSERT INTO, INSERT OR REPLACE, RETURNING
        ├── update.rs       # UPDATE SET ... WHERE ...
        └── delete.rs       # DELETE FROM ... WHERE ...
```

### Core traits and types

```rust
// types.rs
pub enum SqlType { Null, Integer, Real, Text, Blob, Boolean, Json }

pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
    Boolean(bool),
    Json(serde_json::Value),
}

// Coercion rules follow SQLite type affinity spec
impl Value {
    pub fn coerce_to(&self, target: SqlType) -> Result<Value, SqlError>;
    pub fn sql_type(&self) -> SqlType;
}

// row.rs
pub type RowId = i64;

pub struct Row {
    pub row_id: RowId,
    pub values: Vec<Value>,
}

impl Row {
    pub fn encode(&self) -> Vec<u8>;                           // varint record format
    pub fn decode(data: &[u8], schema: &[Column]) -> Result<Row, SqlError>;
}

// planner/mod.rs
pub enum LogicalPlan {
    Scan    { table: String, alias: Option<String> },
    Filter  { input: Box<LogicalPlan>, predicate: Expr },
    Project { input: Box<LogicalPlan>, columns: Vec<Expr> },
    Join    { left: Box<LogicalPlan>, right: Box<LogicalPlan>, on: Expr },
    Agg     { input: Box<LogicalPlan>, group_by: Vec<Expr>, aggs: Vec<AggExpr> },
    Sort    { input: Box<LogicalPlan>, keys: Vec<SortKey> },
    Limit   { input: Box<LogicalPlan>, limit: u64, offset: u64 },
    Insert  { table: String, columns: Vec<String>, values: Vec<Vec<Expr>>, or_replace: bool },
    Update  { table: String, assignments: Vec<(String, Expr)>, predicate: Option<Expr> },
    Delete  { table: String, predicate: Option<Expr> },
    CreateTable { table: Table },
    CreateIndex { index: Index },
    DropTable   { name: String, if_exists: bool },
}

// executor/mod.rs
#[async_trait]
pub trait Executor: Send {
    fn schema(&self) -> &[Column];
    async fn next(&mut self) -> Result<Option<Row>, SqlError>;
    async fn collect_all(&mut self) -> Result<Vec<Row>, SqlError> { ... } // default impl
}

// lib.rs
pub struct SqlEngine {
    storage: Arc<StorageEngine>,
    catalog: Arc<Mutex<SchemaCatalog>>,
}

impl SqlEngine {
    pub async fn new(storage: Arc<StorageEngine>) -> Result<Self, SqlError>;

    // DDL: CREATE TABLE, CREATE INDEX, DROP TABLE, ALTER TABLE
    pub async fn execute_ddl(&self, sql: &str) -> Result<(), SqlError>;

    // DML/DQL: returns async iterator of rows
    pub async fn query(
        &self,
        sql: &str,
        params: &[Value],
    ) -> Result<Box<dyn Executor>, SqlError>;

    // Convenience: run query and discard rows; returns affected row count
    pub async fn execute(
        &self,
        sql: &str,
        params: &[Value],
    ) -> Result<u64, SqlError>;
}
```

### Tasks — Phase 2

- [ ] Create `squrust-sql/Cargo.toml` (deps: squrust-core, sqlparser, serde_json, async-trait, thiserror, tracing)
- [ ] `error.rs` — `SqlError` variants: `Parse`, `Schema`, `Type`, `Constraint`, `Storage(StorageError)`, `NotFound`, `Ambiguous`
- [ ] `types.rs` — `SqlType`, `Value`, type affinity rules matching SQLite spec, `PartialOrd` for comparisons
- [ ] `row.rs` — `Row`, `RowId`, SQLite record format serialization (type byte + varint payload + value data)
- [ ] `schema/mod.rs` — `Column { name, sql_type, not_null, default, primary_key }`, `Table`, `Index { name, table, columns, unique }`
- [ ] `schema/catalog.rs` — `SchemaCatalog`: wraps the `sqlite_master` table (table name, type, sql text); parse `CREATE TABLE` / `CREATE INDEX` DDL to populate in-memory schema on open
- [ ] `parser.rs` — `parse(sql: &str) -> Result<Vec<Statement>, SqlError>` wrapping `sqlparser::Parser::parse_sql`
- [ ] `planner/resolver.rs` — resolve table/column names, expand `SELECT *`, assign column aliases, type-check predicates
- [ ] `planner/mod.rs` — `Statement → LogicalPlan` conversion; handle `WITH` CTEs as subplan rewriting
- [ ] `planner/optimizer.rs` — predicate push-down through Filter/Join, constant folding, index scan selection when filter matches index prefix
- [ ] `executor/scan.rs` — `TableScan`: open B-tree cursor, yield decoded `Row` per call; `IndexScan`: use index B-tree to get rowids, lookup main table
- [ ] `executor/filter.rs` — `FilterExec`: evaluate expression against row; handle AND/OR short-circuit, NULL propagation
- [ ] `executor/projection.rs` — `ProjectExec`: evaluate select-list expressions, alias columns in output schema
- [ ] `executor/join.rs` — `NestedLoopJoin`: inner join, left outer join; optimize to hash join if inner side fits in 64 MB
- [ ] `executor/aggregate.rs` — `AggExec`: GROUP BY grouping with HashMap, accumulate COUNT/SUM/AVG/MIN/MAX, output one row per group
- [ ] `executor/sort.rs` — `SortExec`: collect all rows into Vec, sort by keys with direction, re-emit
- [ ] `executor/limit.rs` — `LimitExec`: pass-through with counter; drop rows beyond limit, skip offset rows
- [ ] `executor/insert.rs` — `InsertExec`: encode row, get next rowid, call `WriteTx::write_page`, update indexes; handle `INSERT OR REPLACE` via delete + insert
- [ ] `executor/update.rs` — `UpdateExec`: scan, evaluate predicate, re-encode modified row, write back; update affected indexes
- [ ] `executor/delete.rs` — `DeleteExec`: scan, evaluate predicate, free pages, remove index entries
- [ ] `lib.rs` — `SqlEngine::new`, `execute_ddl`, `query`, `execute`
- [ ] SQL compatibility tests: CREATE TABLE, INSERT, SELECT with WHERE/ORDER/LIMIT/GROUP BY, UPDATE, DELETE, basic JOINs, NULLs, type coercions
- [ ] Run against a subset of SQLite's official test scripts (via `sqlite3` TCL testfixture subset ported to Rust integration tests)

---

## Phase 3 — squrust-serde

Traits for typed row mapping. Pure trait definitions + blanket impls — no proc macros yet (those are Phase 7).

### File structure

```
squrust-serde/
├── Cargo.toml
└── src/
    ├── lib.rs              # re-exports everything
    ├── from_row.rs         # FromRow trait + blanket impls
    └── to_params.rs        # ToParams trait + blanket impls
```

### Traits

```rust
// from_row.rs
pub trait FromRow: Sized {
    fn from_row(row: &SqurustRow) -> Result<Self, SqurustError>;
}

// Blanket impls for all primitive types
impl FromRow for i64     { ... }
impl FromRow for i32     { ... }
impl FromRow for f64     { ... }
impl FromRow for f32     { ... }
impl FromRow for String  { ... }
impl FromRow for bool    { ... }
impl FromRow for Vec<u8> { ... }
impl FromRow for serde_json::Value { ... }
impl<T: FromRow> FromRow for Option<T> { ... }  // maps NULL → None

// to_params.rs
pub trait ToParams {
    fn to_params(&self) -> Vec<Value>;
}

// Blanket impls
impl ToParams for ()               { fn to_params(&self) -> Vec<Value> { vec![] } }
impl ToParams for Vec<Value>       { fn to_params(&self) -> Vec<Value> { self.clone() } }
impl<T: Into<Value> + Clone> ToParams for &[T] { ... }

// Tuple impls up to 12 elements (T0, T1, ... T11)
impl<T0: Into<Value>, T1: Into<Value>> ToParams for (T0, T1) { ... }
// etc.
```

### Tasks — Phase 3

- [ ] Create `squrust-serde/Cargo.toml` (deps: squrust-sql, thiserror)
- [ ] `from_row.rs` — `FromRow` trait, blanket impls for all primitive types, `Option<T>` impl for NULL
- [ ] `to_params.rs` — `ToParams` trait, impls for `()`, `Vec<Value>`, `&[T]`, tuples up to 12 elements
- [ ] `Value::from` impls for `i64`, `i32`, `f64`, `String`, `bool`, `Vec<u8>`, `Option<T>`
- [ ] Tests: all type round-trips, NULL mapping to `Option::None`

---

## Phase 4 — squrust-async

The primary Rust user-facing API. Wraps `squrust-sql` with async ergonomics, connection pool, streaming results, and typed queries.

### File structure

```
squrust-async/
├── Cargo.toml
└── src/
    ├── lib.rs              # pub use everything; docs
    ├── error.rs            # SqurustError wrapping all lower-level errors
    ├── connection.rs       # SqurustConnection
    ├── pool.rs             # SqurustPool (deadpool)
    ├── query.rs            # Query<'a> builder
    ├── row.rs              # SqurustRow: typed column accessors
    ├── stream.rs           # RowStream<T>: impl Stream<Item = Result<T>>
    ├── transaction.rs      # Transaction
    └── migrate.rs          # Migration runner (without macro support)
```

### API surface

```rust
// connection.rs
pub struct SqurustConnection { inner: Arc<SqlEngine> }

impl SqurustConnection {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, SqurustError>;
    pub async fn open_memory() -> Result<Self, SqurustError>;

    /// Start a query builder
    pub fn query<'a>(&'a self, sql: &'a str) -> Query<'a>;

    /// Execute SQL with no returned rows (INSERT/UPDATE/DELETE/DDL)
    /// Returns rows affected
    pub async fn execute(&self, sql: &str, params: impl ToParams) -> Result<u64, SqurustError>;

    /// Begin a transaction
    pub async fn begin(&self) -> Result<Transaction<'_>, SqurustError>;

    /// Run pending migrations in order
    pub async fn migrate(&self, migrations: &[Migration]) -> Result<(), SqurustError>;
}

// query.rs
pub struct Query<'a> {
    conn:   &'a SqurustConnection,
    sql:    &'a str,
    params: Vec<Value>,
}

impl<'a> Query<'a> {
    pub fn bind(mut self, value: impl Into<Value>) -> Self;

    pub async fn fetch_all<T: FromRow>(self) -> Result<Vec<T>, SqurustError>;
    pub async fn fetch_one<T: FromRow>(self) -> Result<T, SqurustError>;
    pub async fn fetch_optional<T: FromRow>(self) -> Result<Option<T>, SqurustError>;

    /// Returns a Stream — rows are yielded lazily from the executor
    pub fn fetch_stream<T: FromRow + Unpin + 'static>(self) -> RowStream<T>;

    /// For INSERT/UPDATE/DELETE: returns rows affected
    pub async fn execute(self) -> Result<u64, SqurustError>;
}

// row.rs
pub struct SqurustRow { inner: Row }

impl SqurustRow {
    /// Get column by index, typed
    pub fn get<T: FromRow>(&self, idx: usize) -> Result<T, SqurustError>;

    /// Get column by name, typed
    pub fn get_by_name<T: FromRow>(&self, name: &str) -> Result<T, SqurustError>;

    pub fn column_count(&self) -> usize;
    pub fn column_name(&self, idx: usize) -> Option<&str>;
}

// stream.rs
pub struct RowStream<T> { executor: Box<dyn Executor + Send>, _phantom: PhantomData<T> }

impl<T: FromRow + Unpin> Stream for RowStream<T> {
    type Item = Result<T, SqurustError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>>;
}

// transaction.rs
pub struct Transaction<'a> { conn: &'a SqurustConnection }

impl<'a> Transaction<'a> {
    pub fn query(&self, sql: &str) -> Query<'_>;
    pub async fn execute(&self, sql: &str, params: impl ToParams) -> Result<u64, SqurustError>;
    pub async fn commit(self) -> Result<(), SqurustError>;
    pub async fn rollback(self);
}

// pool.rs
pub struct SqurustPool { inner: deadpool::Pool<SqurustManager> }

impl SqurustPool {
    pub async fn new(path: impl AsRef<Path>, max_size: usize) -> Result<Self, SqurustError>;
    pub async fn get(&self) -> Result<PooledConnection<'_>, SqurustError>;
    pub fn close(&self);
}

// migrate.rs
pub struct Migration {
    pub version:     u32,
    pub description: &'static str,
    pub sql:         &'static str,
}

// Runtime migration runner (the migrate!() macro generates the Vec<Migration>)
pub async fn run_migrations(
    conn: &SqurustConnection,
    migrations: &[Migration],
) -> Result<(), SqurustError>;
```

### Tasks — Phase 4

- [ ] Create `squrust-async/Cargo.toml` (deps: squrust-core, squrust-sql, squrust-serde, tokio, futures, async-stream, deadpool, thiserror, tracing)
- [ ] `error.rs` — `SqurustError` wrapping `StorageError`, `SqlError`, pool errors; impl `std::error::Error`
- [ ] `row.rs` — `SqurustRow`, `get::<T>()` and `get_by_name::<T>()` using `FromRow` trait
- [ ] `connection.rs` — `open()`, `open_memory()`, `query()`, `execute()`, `begin()`, `migrate()`
- [ ] `query.rs` — `Query` builder, `bind()` chain, all `fetch_*` and `execute` variants
- [ ] `stream.rs` — `RowStream<T>` as waker-compatible `impl Stream` using `async-stream` macro
- [ ] `transaction.rs` — `Transaction` wrapping `WriteTx`, forwarding query/execute, commit/rollback
- [ ] `pool.rs` — deadpool `Manager` impl creating `SqurustConnection` objects; `SqurustPool` wrapper
- [ ] `migrate.rs` — `Migration` struct, `run_migrations()`: create `_squrust_migrations` table if not exists, apply only unapplied versions in order
- [ ] Integration tests: open in-memory DB, run migrations, INSERT 1000 rows, SELECT with `fetch_all::<MyStruct>`, test connection pool with 20 concurrent tasks
- [ ] Test `Transaction`: insert inside tx, rollback, verify no rows; insert, commit, verify rows present

---

## Phase 5 — squrust-sync

Blocking wrapper. No new logic. Every method calls `tokio::task::block_in_place(|| Handle::current().block_on(...))`.

### Tasks — Phase 5

- [ ] Create `squrust-sync/Cargo.toml` (deps: squrust-async, tokio)
- [ ] `SyncConnection` — mirrors `SqurustConnection` with blocking signatures
- [ ] `SyncQuery` — mirrors `Query` with blocking `fetch_all`, `fetch_one`, `fetch_optional`, `execute`
- [ ] `SyncTransaction` — mirrors `Transaction` with blocking commit/rollback
- [ ] `SyncPool` — wraps `SqurustPool` with blocking `get()`
- [ ] If no tokio runtime is active, `SyncConnection::open()` creates a `tokio::runtime::Runtime` internally (single-threaded)
- [ ] Tests: full CRUD without any `async` in test code

---

## Phase 6 — squrust-ffi

C ABI drop-in for `libsqlite3`. Compiles to both `cdylib` (`.so`) and `staticlib` (`.a`).

> **Critical:** This crate creates its own `tokio::runtime::Runtime` per connection (`current_thread`) so callers need no tokio dependency.

### File structure

```
squrust-ffi/
├── Cargo.toml              # crate-type = ["cdylib", "staticlib"]
├── build.rs                # copy squrust.h to OUT_DIR
├── squrust.h               # C header, API-identical to sqlite3.h
└── src/
    ├── lib.rs
    ├── constants.rs        # SQLITE_OK, SQLITE_ERROR, SQLITE_ROW, etc.
    ├── types.rs            # opaque sqlite3* and sqlite3_stmt* C handles
    ├── state.rs            # per-connection Rust state behind the opaque pointer
    ├── open.rs             # sqlite3_open, sqlite3_open_v2, sqlite3_close, sqlite3_close_v2
    ├── exec.rs             # sqlite3_exec
    ├── prepare.rs          # sqlite3_prepare_v2, sqlite3_finalize, sqlite3_sql
    ├── step.rs             # sqlite3_step, sqlite3_reset, sqlite3_clear_bindings
    ├── bind.rs             # sqlite3_bind_* (int, int64, double, text, blob, null, zeroblob)
    ├── column.rs           # sqlite3_column_* (count, type, name, int, int64, double, text, blob, bytes)
    ├── errmsg.rs           # sqlite3_errcode, sqlite3_errmsg, sqlite3_errmsg16
    └── meta.rs             # sqlite3_libversion, sqlite3_changes, sqlite3_last_insert_rowid, sqlite3_interrupt
```

### Required C symbols (all must be `#[no_mangle] pub unsafe extern "C"`)

```rust
// constants.rs — must match sqlite3.h exactly
pub const SQLITE_OK:       c_int = 0;
pub const SQLITE_ERROR:    c_int = 1;
pub const SQLITE_INTERNAL: c_int = 2;
pub const SQLITE_PERM:     c_int = 3;
pub const SQLITE_ABORT:    c_int = 4;
pub const SQLITE_BUSY:     c_int = 5;
pub const SQLITE_LOCKED:   c_int = 6;
pub const SQLITE_NOMEM:    c_int = 7;
pub const SQLITE_READONLY: c_int = 8;
pub const SQLITE_IOERR:    c_int = 10;
pub const SQLITE_CORRUPT:  c_int = 11;
pub const SQLITE_NOTFOUND: c_int = 12;
pub const SQLITE_FULL:     c_int = 13;
pub const SQLITE_CANTOPEN: c_int = 14;
pub const SQLITE_MISUSE:   c_int = 21;
pub const SQLITE_ROW:      c_int = 100;
pub const SQLITE_DONE:     c_int = 101;

// Column types
pub const SQLITE_INTEGER: c_int = 1;
pub const SQLITE_FLOAT:   c_int = 2;
pub const SQLITE_TEXT:    c_int = 3;
pub const SQLITE_BLOB:    c_int = 4;
pub const SQLITE_NULL:    c_int = 5;

// open.rs
#[no_mangle] pub unsafe extern "C" fn sqlite3_open(filename: *const c_char, ppDb: *mut *mut Sqlite3) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_open_v2(filename: *const c_char, ppDb: *mut *mut Sqlite3, flags: c_int, zVfs: *const c_char) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_close(db: *mut Sqlite3) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_close_v2(db: *mut Sqlite3) -> c_int;

// exec.rs
type sqlite3_callback = Option<unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int>;
#[no_mangle] pub unsafe extern "C" fn sqlite3_exec(db: *mut Sqlite3, sql: *const c_char, cb: sqlite3_callback, arg: *mut c_void, errmsg: *mut *mut c_char) -> c_int;

// prepare.rs
#[no_mangle] pub unsafe extern "C" fn sqlite3_prepare_v2(db: *mut Sqlite3, sql: *const c_char, nByte: c_int, ppStmt: *mut *mut Sqlite3Stmt, pzTail: *mut *const c_char) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_finalize(stmt: *mut Sqlite3Stmt) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_sql(stmt: *mut Sqlite3Stmt) -> *const c_char;
#[no_mangle] pub unsafe extern "C" fn sqlite3_stmt_readonly(stmt: *mut Sqlite3Stmt) -> c_int;

// step.rs
#[no_mangle] pub unsafe extern "C" fn sqlite3_step(stmt: *mut Sqlite3Stmt) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_reset(stmt: *mut Sqlite3Stmt) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_clear_bindings(stmt: *mut Sqlite3Stmt) -> c_int;

// bind.rs — all must be present
#[no_mangle] pub unsafe extern "C" fn sqlite3_bind_int(stmt: *mut Sqlite3Stmt, col: c_int, val: c_int) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_bind_int64(stmt: *mut Sqlite3Stmt, col: c_int, val: i64) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_bind_double(stmt: *mut Sqlite3Stmt, col: c_int, val: f64) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_bind_text(stmt: *mut Sqlite3Stmt, col: c_int, val: *const c_char, n: c_int, destructor: Option<unsafe extern "C" fn(*mut c_void)>) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_bind_blob(stmt: *mut Sqlite3Stmt, col: c_int, val: *const c_void, n: c_int, destructor: Option<unsafe extern "C" fn(*mut c_void)>) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_bind_null(stmt: *mut Sqlite3Stmt, col: c_int) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_bind_zeroblob(stmt: *mut Sqlite3Stmt, col: c_int, n: c_int) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_bind_parameter_count(stmt: *mut Sqlite3Stmt) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_bind_parameter_name(stmt: *mut Sqlite3Stmt, col: c_int) -> *const c_char;

// column.rs — all must be present
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_count(stmt: *mut Sqlite3Stmt) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_type(stmt: *mut Sqlite3Stmt, col: c_int) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_name(stmt: *mut Sqlite3Stmt, col: c_int) -> *const c_char;
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_int(stmt: *mut Sqlite3Stmt, col: c_int) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_int64(stmt: *mut Sqlite3Stmt, col: c_int) -> i64;
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_double(stmt: *mut Sqlite3Stmt, col: c_int) -> f64;
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_text(stmt: *mut Sqlite3Stmt, col: c_int) -> *const c_uchar;
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_blob(stmt: *mut Sqlite3Stmt, col: c_int) -> *const c_void;
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_bytes(stmt: *mut Sqlite3Stmt, col: c_int) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_column_decltype(stmt: *mut Sqlite3Stmt, col: c_int) -> *const c_char;

// errmsg.rs
#[no_mangle] pub unsafe extern "C" fn sqlite3_errcode(db: *mut Sqlite3) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_errmsg(db: *mut Sqlite3) -> *const c_char;
#[no_mangle] pub unsafe extern "C" fn sqlite3_errmsg16(db: *mut Sqlite3) -> *const c_void;
#[no_mangle] pub unsafe extern "C" fn sqlite3_free(ptr: *mut c_void);

// meta.rs
#[no_mangle] pub unsafe extern "C" fn sqlite3_libversion() -> *const c_char;
#[no_mangle] pub unsafe extern "C" fn sqlite3_libversion_number() -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_changes(db: *mut Sqlite3) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_changes64(db: *mut Sqlite3) -> i64;
#[no_mangle] pub unsafe extern "C" fn sqlite3_last_insert_rowid(db: *mut Sqlite3) -> i64;
#[no_mangle] pub unsafe extern "C" fn sqlite3_interrupt(db: *mut Sqlite3);
#[no_mangle] pub unsafe extern "C" fn sqlite3_get_autocommit(db: *mut Sqlite3) -> c_int;
#[no_mangle] pub unsafe extern "C" fn sqlite3_complete(sql: *const c_char) -> c_int;
```

### Tasks — Phase 6

- [ ] Create `squrust-ffi/Cargo.toml`: `crate-type = ["cdylib", "staticlib"]`, deps: squrust-sql, squrust-sync, tokio (rt), libc, thiserror
- [ ] `constants.rs` — all `SQLITE_*` constants matching sqlite3.h verbatim (copy from sqlite3.h source)
- [ ] `types.rs` — opaque `Sqlite3` and `Sqlite3Stmt` C structs; both are `repr(C)` with a `Box<ConnectionState>` / `Box<StmtState>` inside
- [ ] `state.rs` — `ConnectionState`: holds `SyncConnection`, last error string, last rowid, last changes count; `StmtState`: holds prepared SQL, current result row, bound params
- [ ] `open.rs` — `sqlite3_open`, `sqlite3_open_v2` (handle `:memory:` flag), `sqlite3_close`, `sqlite3_close_v2` — Box/unbox state correctly with `Box::from_raw` / `Box::into_raw`
- [ ] `prepare.rs` — `sqlite3_prepare_v2`: parse + plan the SQL, store in `StmtState`; `sqlite3_finalize`: drop StmtState; `sqlite3_sql`
- [ ] `step.rs` — `sqlite3_step`: advance executor, cache current row in `StmtState`, return `SQLITE_ROW` or `SQLITE_DONE`; `sqlite3_reset`, `sqlite3_clear_bindings`
- [ ] `bind.rs` — all bind variants: convert C types to `Value`, store in `StmtState.params`
- [ ] `column.rs` — all column accessors: pull from `StmtState.current_row`, convert to C types, cache text/blob in StmtState for pointer lifetime
- [ ] `exec.rs` — `sqlite3_exec`: parse, execute, call C callback for each row
- [ ] `errmsg.rs` — `sqlite3_errmsg`: return pointer to error string cached in `ConnectionState`; `sqlite3_free` for allocated C strings
- [ ] `meta.rs` — `sqlite3_libversion` returning `"3.45.0 (squrust)"`, `sqlite3_changes`, `sqlite3_last_insert_rowid`, `sqlite3_interrupt` (set cancel flag)
- [ ] `squrust.h` — C header file with all the above declarations; should be includable as a drop-in for `sqlite3.h`
- [ ] Verification: `LD_PRELOAD=./libsqurust.so python3 -c "import sqlite3; db=sqlite3.connect(':memory:'); db.execute('CREATE TABLE t(x); INSERT INTO t VALUES (1); SELECT * FROM t')"` passes
- [ ] Verification: `LD_PRELOAD=./libsqurust.so node -e "const db = require('better-sqlite3')(':memory:'); db.prepare('CREATE TABLE t(x)').run()"` passes
- [ ] Verification: PHP PDO sqlite driver works with LD_PRELOAD

---

## Phase 7 — squrust-macros

Proc macro crate. Must be a separate crate from all others (Rust requirement for proc-macros).

### Tasks — Phase 7

- [ ] Create `squrust-macros/Cargo.toml`: `[lib] proc-macro = true`, deps: syn (full features), quote, proc-macro2, squrust-serde (for trait paths)
- [ ] `#[derive(FromRow)]`:
  - For each named field in the struct, generate code: `row.get_by_name::<FieldType>(stringify!(#field_name))?`
  - Support `#[squrust(rename = "column_name")]` attribute to override column name
  - Support `#[squrust(skip)]` to skip a field (use `Default::default()`)
  - Emit compile error if field type doesn't implement `FromRow`
- [ ] `#[derive(ToParams)]`:
  - For each field, emit `Value::from(self.#field_name.clone())` into a Vec
  - Support `#[squrust(skip)]` to omit from params
- [ ] `sql!()` macro:
  - Read `SQURUST_SCHEMA` env var (path to schema file) or look for `squrust.schema` in `CARGO_MANIFEST_DIR`
  - Parse the SQL string at compile time using `sqlparser`
  - Validate table names and column names against the schema file
  - Emit a compile error with the invalid identifier highlighted if validation fails
  - Return a `Query` value (does not need to be typed yet — typed inference is a Phase 2 enhancement)
- [ ] `migrate!("./migrations")` macro:
  - At compile time, read all `.sql` files in the given path (relative to `CARGO_MANIFEST_DIR`)
  - Sort by filename prefix numerically
  - Embed as `&[Migration]` with version, description (from filename), and SQL content as `&'static str`
  - Emit compile error if any file has a duplicate version prefix
- [ ] Compile-fail tests using `trybuild`: invalid column in `sql!()`, wrong derive type, missing schema file

---

## Phase 8 — squrust-wasm

Browser target using OPFS (Origin Private File System). Alternate `StorageEngine` implementation, same `squrust-sql` on top.

### Tasks — Phase 8

- [ ] Create `squrust-wasm/Cargo.toml` (deps: squrust-sql, wasm-bindgen, js-sys, web-sys with features: ["WorkerGlobalScope", "FileSystemFileHandle", "FileSystemSyncAccessHandle", "StorageManager"], getrandom with "js" feature)
- [ ] `opfs_storage.rs` — implement the `StorageEngine` trait backed by OPFS `FileSystemSyncAccessHandle` (synchronous OPFS API, must run in a Worker)
- [ ] `lib.rs` — wasm-bindgen exported class `SqurustDb`:
  ```typescript
  // TypeScript interface
  class SqurustDb {
      static open(name: string): Promise<SqurustDb>;
      static openMemory(): Promise<SqurustDb>;
      query(sql: string, params?: unknown[]): Promise<Record<string, unknown>[]>;
      execute(sql: string, params?: unknown[]): Promise<number>;
      close(): Promise<void>;
  }
  ```
- [ ] `package.json` for the npm package `squrust`
- [ ] `index.d.ts` TypeScript type definitions
- [ ] Test: open in-memory DB in headless browser (via `wasm-pack test --headless --chrome`)
- [ ] Test: OPFS persistence test — open, write, close, reopen, verify data

---

## Phase 9 — squrust-cli

`sq` binary, drop-in for `sqlite3` CLI.

### Tasks — Phase 9

- [ ] Create `squrust-cli/Cargo.toml` (deps: squrust-async, clap (derive), rustyline, comfy-table, serde_json, csv, tokio, anyhow, tracing-subscriber)
- [ ] `main.rs` — entry point: parse `sq [database] [sql]` args via clap; if SQL arg given, run it and exit; else start REPL
- [ ] REPL: `rustyline::Editor` with history file at `~/.sq_history`; handle multi-line input (buffer until `;` terminator)
- [ ] Output modes: `--mode table` (default, comfy-table), `--mode csv`, `--mode json`, `--mode line`
- [ ] Dot-commands: `.schema [table]`, `.tables`, `.mode MODE`, `.output [file]`, `.import FILE TABLE`, `.help`, `.exit`, `.quit`
- [ ] `.import`: parse CSV file, infer or use existing schema, bulk INSERT
- [ ] Pipe mode: `echo "SELECT 1" | sq mydb.db` — detect non-TTY stdin, read SQL from stdin
- [ ] Test: `sq :memory: "CREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1),(2); SELECT * FROM t"` outputs table

---

## Testing strategy

### Unit tests — per crate

Every crate has `#[cfg(test)]` modules in each source file covering its internal logic.

### Integration tests — workspace root `tests/`

```
tests/
├── sqlite_compat.rs    # SQL compatibility: tables, CRUD, types, NULL, expressions
├── concurrent.rs       # 50 concurrent writers, verify no corruption or deadlock
├── crash_recovery.rs   # interrupt mid-write, checkpoint, verify WAL recovery
├── ffi_compat.rs       # call squrust-ffi C symbols from Rust unsafe tests
└── migration.rs        # run migrations on fresh DB, verify version tracking
```

### Property tests — `proptest`

- `squrust-core`: random key sequences inserted into B-tree, verify sorted order invariant holds
- `squrust-sql`: random valid SQL expressions, verify no panics (only `SqlError` results)

### Fuzz targets — `fuzz/`

```
fuzz/
├── Cargo.toml
└── fuzz_targets/
    ├── fuzz_sql.rs     # random SQL strings → squrust-sql; must not panic
    ├── fuzz_page.rs    # random page bytes → squrust-core decoder; must not panic
    └── fuzz_ffi.rs     # random sqlite3_* C call sequences; must not segfault
```

Run: `cargo +nightly fuzz run fuzz_sql -- -max_total_time=3600`

### Benchmarks — `benches/`

```
benches/
└── vs_sqlite.rs        # criterion: squrust vs sqlite3 on read-heavy, write-heavy, concurrent workloads
```

Targets:
- Read throughput: ≥ SQLite
- Single-writer throughput: ≥ 80% of SQLite
- Concurrent write throughput: ≥ 3× SQLite (MVCC wins here)

---

## Invariants — the agent must never violate these

1. **Dependency direction is one-way and downward:** `cli/wasm → async → sql → core`. No crate may import a crate above it in the stack. `squrust-ffi` imports `sql` and `core` only (no `async`).

2. **No `unsafe` except in two places:** `squrust-core/src/storage/file.rs` (mmap), and all of `squrust-ffi/src/` (C ABI boundary). Every other file must compile with `#![forbid(unsafe_code)]`.

3. **`squrust-ffi` is self-contained at the C boundary:** It creates its own `tokio::runtime::Builder::new_current_thread().build()` per connection. Callers need zero knowledge of async or tokio.

4. **SQLite file format compatibility is non-negotiable in Phase 1:** The 100-byte header must match byte-for-byte. The B-tree page format must produce files readable by `sqlite3`. Verify with: `sqlite3 test.db ".schema"` on a DB written by `squrust-core`.

5. **All public library errors use `thiserror`.** `anyhow` is allowed only in `squrust-cli` and test code.

6. **MSRV: Rust 1.80.** Every crate root must have `#![deny(warnings)]`.

7. **The C ABI symbol list is exhaustive.** If a program using `libsqlite3` calls a symbol not in the list, it will segfault on LD_PRELOAD. Add any missing symbols as stubs returning `SQLITE_OK` or `SQLITE_MISUSE` before shipping Phase 6.

8. **`cargo clippy -- -D warnings` must pass clean on every crate before moving to the next phase.**
