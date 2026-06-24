# Storage Engine (`squrust-core`)

`squrust-core` is the foundation: a `unsafe`-free, synchronous storage engine.
It knows nothing about SQL — it deals in pages, a B-tree, a write-ahead log, and
MVCC transactions.

```
StorageEngine
 ├── DatabaseFile   positioned pread/pwrite over the .db file
 ├── PageCache      LRU cache of clean pages read from the main file
 ├── WriteAheadLog  committed page versions (durability + snapshot reads)
 └── MvccManager    version counter + active-reader set
```

## Pages

- Fixed **4096-byte** pages (`PAGE_SIZE`), 1-based `PageId`. Page 1 holds the
  100-byte database header followed by the `sqlite_master` B-tree.
- `RawPage { id, data: Box<[u8; 4096]>, dirty }`.
- I/O is done with `std::os::unix::fs::FileExt` (`read_exact_at` / `write_all_at`)
  — no `mmap`, hence no `unsafe`.

## Page cache (`page/cache.rs`)

A thread-safe LRU cache (default 2000 pages ≈ 8 MiB) of **clean** pages read
from the main file. Entries are `Arc<RawPage>`: a reader keeps a clone alive even
after the cache evicts its own reference.

## Write-ahead log (`wal/`)

Squrust's WAL is its **own format** (not SQLite's WAL), stored beside the
database as `<db>-squrust-wal`:

- Each frame is a 32-byte header (page id, commit version, db-size-after, salt,
  FNV checksum) plus the 4096-byte page image.
- Committed page versions are appended **and** kept in an in-memory index so
  reads can find the right version for a snapshot without touching the main
  file. This is what gives snapshot isolation.
- **Crash recovery:** on open, the WAL is replayed; a trailing half-written
  (uncommitted) transaction is detected via the checksum/commit marker and
  truncated.
- **Checkpoint:** `checkpoint()` writes the latest committed version of every
  page into the main file, updates the header, fsyncs, and truncates the WAL.
  After a checkpoint the main `.db` is a complete, stock-`sqlite3`-readable file.

> Because the WAL is Squrust's own format, full crash-recovery interchange with
> `sqlite3` isn't a goal; the **checkpointed main file** is what's interoperable.

## MVCC (`mvcc/`)

Snapshot isolation, the same discipline SQLite uses in WAL mode:

- A monotonic `AtomicU64` hands out **commit versions**.
- `begin_read()` pins the current committed version; the reader only ever sees
  page versions `≤` that, served from the WAL index (newer) or the main file
  (older/checkpointed).
- `begin_write()` acquires a single-writer gate.
- `MvccManager` tracks active readers so a checkpoint knows the oldest version
  still required.

```rust
// snapshot isolation in action (paraphrased from the core tests)
let reader = engine.begin_read();          // pin version V
// ... a writer commits a new row at version V+1 ...
assert!(tree.get(&reader, new_id)?.is_none());  // reader still sees only ≤ V
```

## B-tree (`btree/`)

A **table B-tree** mapping `i64` rowids → record bytes, in the **real SQLite
on-disk page format** (see [[SQLite Compatibility]]):

- Leaf pages `0x0d`, interior pages `0x05`, with the standard cell-pointer array
  and cell content area.
- Large records spill into **overflow page** chains.
- Splits propagate up; the root keeps a **stable page id** across root splits
  (its content moves to a fresh child) — which is what lets the catalog live
  permanently on page 1.
- A `BTreeCursor` does in-order traversal (`seek`, `seek_first`, `next`).

The B-tree operates over `PageSource` / `PageSink` traits, implemented by the
transactions:

```rust
pub trait PageSource { fn get_page(&self, id: PageId) -> Result<Arc<RawPage>>; }
pub trait PageSink: PageSource {
    fn alloc_page(&self) -> Result<PageId>;
    fn put_page(&self, page: RawPage) -> Result<()>;
    fn free_page(&self, id: PageId) -> Result<()>;
}
```

## Transactions

```rust
let engine = StorageEngine::open(path)?;      // Arc<StorageEngine>

// read snapshot
let rtx = engine.begin_read();
let tree = BTree::open(root_page);
let value = tree.get(&rtx, rowid)?;

// write transaction (single writer)
let wtx = engine.begin_write();
let tree = BTree::open(root_page);
tree.insert(&wtx, rowid, &record_bytes)?;     // buffered
wtx.commit()?;                                 // atomic WAL append + fsync
```

- `WriteTx` buffers dirty pages in a local map and tracks page-count/freelist
  changes; `commit()` appends one fsync'd WAL batch and publishes the new
  version; `rollback()` (or drop) discards.
- `commit`/`rollback` take `&self`, so an `Arc<WriteTx>` can back a long-lived
  transaction (used by the [[Async API]] and [[C ABI and LD_PRELOAD]] layers to
  read a transaction's own uncommitted writes).

## Notable simplifications

- Page size is fixed at 4096.
- The freelist isn't persisted across reopen (freed pages may leak space, but
  `PRAGMA integrity_check` still passes). See [[Roadmap]].

See also: [[Architecture]], [[SQL Engine]], [[SQLite Compatibility]].
