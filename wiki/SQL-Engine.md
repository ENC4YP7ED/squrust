# SQL Engine (`squrust-sql`)

`squrust-sql` turns SQL text into rows, on top of [[Storage Engine]]
transactions. It owns the value model, the schema catalog, the planner, and a
volcano-style executor.

```
&str ─(sqlparser)─► Statement ─(planner)─► Plan ─(optimizer)─► executor tree ─► Row
```

The entry point is `SqlEngine`:

```rust
pub struct SqlEngine { /* storage + catalog + counters */ }

impl SqlEngine {
    pub async fn new(storage: Arc<StorageEngine>) -> Result<Arc<SqlEngine>>;
    pub async fn execute(&self, sql: &str, params: &[Value]) -> Result<u64>;       // DML/DDL
    pub async fn query(&self, sql: &str, params: &[Value]) -> Result<Box<dyn Executor>>; // SELECT
    pub fn build_query(&self, src: ReadSource, sql: &str, params: &[Value]) -> Result<Box<dyn Executor>>;
    pub async fn execute_on(&self, tx: &WriteTx, sql: &str, params: &[Value]) -> Result<u64>; // DML in a txn
}
```

## Values & type affinity (`types.rs`)

```rust
pub enum Value { Null, Integer(i64), Real(f64), Text(String), Blob(Vec<u8>), Boolean(bool), Json(serde_json::Value) }
```

`Value::coerce_to(affinity)` implements SQLite's
[column affinity](https://www.sqlite.org/datatype3.html#affinity) rules
faithfully:

- **BLOB / NONE** affinity (e.g. a typeless column) does **no** conversion — an
  integer stays an integer.
- **INTEGER / NUMERIC** affinity keeps a fractional real as real (no truncation)
  and converts numeric-looking text.
- **TEXT** converts numbers to text but leaves blobs alone.

Comparisons follow SQLite's storage-class ordering (NULL < numbers < text <
blob), with NULL propagation for predicates.

## Row encoding (`row.rs`)

Rows are stored as **SQLite records** (serial-type header + value bodies, with
varints) so the bytes are readable by `sqlite3`. See
[[SQLite Compatibility]] for the wire format. The rowid-alias column
(`INTEGER PRIMARY KEY`) is stored as NULL and recovered from the B-tree key on
read — exactly as SQLite does.

## Schema catalog (`schema/`)

The catalog is `sqlite_master` itself, rooted at **page 1**. On open, Squrust
scans page 1 and parses each table's stored `CREATE TABLE` text to rebuild the
in-memory schema. `CREATE TABLE` allocates a fresh B-tree root and inserts a
`sqlite_master` row; `DROP TABLE` removes it.

## Planner (`planner/`)

- `resolver.rs` resolves identifiers against a column **scope**, translates
  `sqlparser` expressions into a resolved `Expr` (columns → indices, `?` →
  positional params), expands `SELECT *`, and lowers `BETWEEN`, `CASE`, `CAST`,
  `TRIM`, etc.
- `mod.rs` builds the `LogicalPlan`:
  `Scan`, `Filter`, `Project`, `NestedLoopJoin`, `Aggregate`, `Distinct`,
  `Sort`, `Limit`,
  plus statement plans for `Insert`/`Update`/`Delete`/`CreateTable`/`CreateIndex`/`DropTable`.
- `optimizer.rs` does constant folding (deliberately *not* folding `/` and `%`,
  to leave SQLite's integer-vs-real division to the evaluator).

## Executors (`executor/`)

A pull-based **volcano** model — each operator implements:

```rust
#[async_trait]
pub trait Executor: Send {
    fn columns(&self) -> &[ColumnInfo];
    async fn next(&mut self) -> Result<Option<Row>>;
}
```

Operators: `TableScan`, `FilterExec`, `ProjectExec`, `NestedLoopJoin`,
`AggExec` (COUNT/SUM/AVG/MIN/MAX/GROUP_CONCAT + GROUP BY + HAVING), `DistinctExec`,
`SortExec`, `LimitExec`, and a `DualExec` for `SELECT <expr>` with no `FROM`. `eval.rs` is the expression
evaluator (arithmetic with three-valued logic, `LIKE`, `IN`, `CASE`, `CAST`, and
the scalar functions listed in [[SQLite Compatibility]], plus SQLite-compatible
date/time functions in `datetime.rs`). DML lives in `dml.rs`
and runs directly against a `WriteTx`.

## Supported SQL (today)

- `SELECT` / `SELECT DISTINCT` with `WHERE`, `ORDER BY` (incl. by ordinal/alias),
  `LIMIT`/`OFFSET`, `GROUP BY` + aggregates + `HAVING`, expressions & scalar
  functions, `CASE`, `CAST`.
- `INSERT` (incl. `INSERT OR REPLACE`, multi-row `VALUES`), `UPDATE`, `DELETE`.
- `CREATE TABLE` / `CREATE INDEX` / `DROP TABLE` / `ALTER TABLE … ADD COLUMN`.
- Inner, left-outer, cross, and comma joins over any number of tables
  (left-deep nested-loop tree).
- `BEGIN` / `COMMIT` / `ROLLBACK` (real transactions through the [[Async API]]
  and [[C ABI and LD_PRELOAD]]).
- Row-returning `PRAGMA`s: `table_info`, `foreign_keys`, `user_version`,
  `journal_mode` (parsed by a dedicated `pragma` module, since the SQL grammar
  rejects `table_info(t)`'s unquoted argument).

See [[SQLite Compatibility]] for what's **not** yet supported (index usage,
correlated subqueries, RIGHT/FULL joins, window/user functions, …).
