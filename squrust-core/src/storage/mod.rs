//! The storage engine: ties pages, cache, WAL and MVCC into transactions.
//!
//! Concurrency: a single writer at a time (serialised by a write gate) and any
//! number of concurrent readers. A write transaction buffers all of its page
//! changes locally and flushes them to the WAL atomically at commit, so readers
//! never observe a partially-applied transaction.

pub mod file;
pub mod header;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use parking_lot::Mutex as PlMutex;

use crate::btree::{PageSink, PageSource};
use crate::error::{Result, StorageError};
use crate::mvcc::{MvccManager, Snapshot, TransactionId};
use crate::page::cache::PageCache;
use crate::page::{PAGE_SIZE, PageId, RawPage};
use crate::wal::{WalFrame, WriteAheadLog};

use file::DatabaseFile;

/// Single-writer gate. Only one write transaction may be active at a time.
struct WriteGate {
    active: Mutex<bool>,
    cv: Condvar,
}

impl WriteGate {
    fn new() -> Self {
        WriteGate {
            active: Mutex::new(false),
            cv: Condvar::new(),
        }
    }
    fn acquire(&self) {
        let mut guard = self.active.lock().unwrap();
        while *guard {
            guard = self.cv.wait(guard).unwrap();
        }
        *guard = true;
    }
    fn release(&self) {
        let mut guard = self.active.lock().unwrap();
        *guard = false;
        self.cv.notify_one();
    }
}

/// Removes a temporary (`open_memory`) database's files when the engine drops.
struct TempFileGuard {
    db: PathBuf,
    wal: PathBuf,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.db);
        let _ = std::fs::remove_file(&self.wal);
    }
}

static MEM_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct StorageEngine {
    file: Arc<DatabaseFile>,
    cache: Arc<PageCache>,
    wal: Arc<WriteAheadLog>,
    mvcc: Arc<MvccManager>,
    gate: WriteGate,
    /// High-water mark for page allocation (number of allocated pages).
    page_count: PlMutex<u32>,
    /// In-memory freelist of reusable page ids (not persisted across reopen).
    freelist: PlMutex<Vec<PageId>>,
    _temp: Option<TempFileGuard>,
}

impl StorageEngine {
    /// Open (creating if missing) a database at `path`.
    pub fn open(path: &Path) -> Result<Arc<StorageEngine>> {
        Self::open_inner(path, None)
    }

    /// Open a transient database backed by a private temporary file that is
    /// removed when the engine is dropped. Behaves like SQLite's `:memory:`.
    pub fn open_memory() -> Result<Arc<StorageEngine>> {
        let n = MEM_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let mut path = std::env::temp_dir();
        path.push(format!("squrust-mem-{pid}-{n}.db"));
        let _ = std::fs::remove_file(&path);
        let wal = with_wal_suffix(&path);
        let _ = std::fs::remove_file(&wal);
        let guard = TempFileGuard {
            db: path.clone(),
            wal,
        };
        Self::open_inner(&path, Some(guard))
    }

    fn open_inner(path: &Path, temp: Option<TempFileGuard>) -> Result<Arc<StorageEngine>> {
        let (file, header) = DatabaseFile::open(path)?;
        let file = Arc::new(file);
        let wal_path = with_wal_suffix(path);
        let wal = Arc::new(WriteAheadLog::open(&wal_path, header.db_size_pages)?);

        let physical = file.page_count()?;
        let page_count = header
            .db_size_pages
            .max(wal.db_size())
            .max(physical)
            .max(1);

        let mvcc = Arc::new(MvccManager::new(wal.max_version()));

        Ok(Arc::new(StorageEngine {
            file,
            cache: Arc::new(PageCache::with_default_capacity()),
            wal,
            mvcc,
            gate: WriteGate::new(),
            page_count: PlMutex::new(page_count),
            freelist: PlMutex::new(Vec::new()),
            _temp: temp,
        }))
    }

    /// Read page `id` as visible at snapshot `version`.
    fn read_page_at(&self, id: PageId, version: TransactionId) -> Result<Arc<RawPage>> {
        if let Some(data) = self.wal.read_page(id, version) {
            return Ok(Arc::new(RawPage::from_bytes(id, &data[..])));
        }
        if let Some(p) = self.cache.get(id) {
            return Ok(p);
        }
        match self.file.read_page(id)? {
            Some(data) => {
                let p = Arc::new(RawPage::from_bytes(id, &data[..]));
                self.cache.insert(p.clone());
                Ok(p)
            }
            None => Err(StorageError::PageOutOfRange(id)),
        }
    }

    pub fn begin_read(self: &Arc<Self>) -> ReadTx {
        let snapshot = self.mvcc.begin_read();
        ReadTx {
            engine: Arc::clone(self),
            snapshot,
        }
    }

    pub fn begin_write(self: &Arc<Self>) -> WriteTx {
        self.gate.acquire();
        let id = self.mvcc.begin_write();
        let snapshot = Snapshot::new(self.mvcc.current_version());
        let page_count = *self.page_count.lock();
        let freelist = self.freelist.lock().clone();
        WriteTx {
            engine: Arc::clone(self),
            id,
            snapshot,
            pending: PlMutex::new(HashMap::new()),
            local_page_count: PlMutex::new(page_count),
            local_freelist: PlMutex::new(freelist),
            finished: PlMutex::new(false),
        }
    }

    /// Fold the WAL back into the main file. No-op if a lagging reader still
    /// requires an older snapshot.
    pub fn checkpoint(&self) -> Result<()> {
        if self.mvcc.oldest_required_version() < self.mvcc.current_version() {
            return Ok(());
        }
        self.wal.checkpoint(&self.file, &self.cache)
    }

    pub fn sync(&self) -> Result<()> {
        self.file.sync()
    }

    pub fn page_count(&self) -> u32 {
        *self.page_count.lock()
    }
}

fn with_wal_suffix(path: &Path) -> PathBuf {
    // Not "-wal": that name belongs to SQLite's own WAL. Ours has its own
    // format, and a checkpointed Squrust file is a plain (journal-mode) SQLite
    // database with no companion WAL.
    let mut s = path.as_os_str().to_os_string();
    s.push("-squrust-wal");
    PathBuf::from(s)
}

/// A read-only transaction pinned to a snapshot.
pub struct ReadTx {
    engine: Arc<StorageEngine>,
    snapshot: Snapshot,
}

impl ReadTx {
    pub fn get_page(&self, id: PageId) -> Result<Arc<RawPage>> {
        self.engine.read_page_at(id, self.snapshot.version)
    }

    pub fn snapshot_version(&self) -> TransactionId {
        self.snapshot.version
    }

    pub fn commit(self) {
        // nothing to flush; dropping releases the snapshot
    }
}

impl Drop for ReadTx {
    fn drop(&mut self) {
        self.engine.mvcc.end_read(self.snapshot);
    }
}

impl PageSource for ReadTx {
    fn get_page(&self, id: PageId) -> Result<Arc<RawPage>> {
        ReadTx::get_page(self, id)
    }
}

/// A read-write transaction. Buffers writes until commit.
pub struct WriteTx {
    engine: Arc<StorageEngine>,
    id: TransactionId,
    snapshot: Snapshot,
    pending: PlMutex<HashMap<PageId, Arc<RawPage>>>,
    local_page_count: PlMutex<u32>,
    local_freelist: PlMutex<Vec<PageId>>,
    finished: PlMutex<bool>,
}

impl WriteTx {
    pub fn id(&self) -> TransactionId {
        self.id
    }

    pub fn get_page(&self, id: PageId) -> Result<Arc<RawPage>> {
        if let Some(p) = self.pending.lock().get(&id) {
            return Ok(p.clone());
        }
        self.engine.read_page_at(id, self.snapshot.version)
    }

    pub fn write_page(&self, page: RawPage) -> Result<()> {
        self.pending.lock().insert(page.id, Arc::new(page));
        Ok(())
    }

    pub fn alloc_page(&self) -> Result<PageId> {
        if let Some(id) = self.local_freelist.lock().pop() {
            return Ok(id);
        }
        let mut pc = self.local_page_count.lock();
        *pc += 1;
        Ok(*pc)
    }

    pub fn free_page(&self, id: PageId) -> Result<()> {
        self.pending.lock().remove(&id);
        self.local_freelist.lock().push(id);
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        {
            let pending = self.pending.lock();
            let version = self.engine.mvcc.reserve_commit_version();
            let db_size = *self.local_page_count.lock();

            if !pending.is_empty() {
                let mut frames: Vec<WalFrame> = Vec::with_capacity(pending.len());
                for page in pending.values() {
                    let mut data = Box::new([0u8; PAGE_SIZE]);
                    data.copy_from_slice(&page.data[..]);
                    frames.push(WalFrame {
                        page_id: page.id,
                        commit_version: version,
                        db_size_after: 0,
                        salt: self.engine.wal.salt(),
                        data,
                    });
                    // Refresh the clean cache so later non-WAL reads are correct.
                    self.engine.cache.invalidate(page.id);
                }
                self.engine.wal.append_commit(frames, version, db_size)?;
            }
            self.engine.mvcc.publish_commit(version);

            *self.engine.page_count.lock() = db_size;
            *self.engine.freelist.lock() = self.local_freelist.lock().clone();
        }
        self.finish();
        Ok(())
    }

    pub fn rollback(&self) {
        self.finish();
    }

    fn finish(&self) {
        let mut done = self.finished.lock();
        if !*done {
            *done = true;
            self.engine.gate.release();
        }
    }
}

impl Drop for WriteTx {
    fn drop(&mut self) {
        // If neither commit nor rollback ran, release the gate (implicit rollback).
        self.finish();
    }
}

impl PageSource for WriteTx {
    fn get_page(&self, id: PageId) -> Result<Arc<RawPage>> {
        WriteTx::get_page(self, id)
    }
}

impl PageSink for WriteTx {
    fn alloc_page(&self) -> Result<PageId> {
        WriteTx::alloc_page(self)
    }
    fn put_page(&self, page: RawPage) -> Result<()> {
        WriteTx::write_page(self, page)
    }
    fn free_page(&self, id: PageId) -> Result<()> {
        WriteTx::free_page(self, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::btree::BTree;

    #[test]
    fn write_read_roundtrip() {
        let engine = StorageEngine::open_memory().unwrap();
        let root = {
            let tx = engine.begin_write();
            let root = BTree::create(&tx).unwrap();
            let tree = BTree::open(root);
            tree.insert(&tx, 1, b"hello").unwrap();
            tree.insert(&tx, 2, b"world").unwrap();
            tx.commit().unwrap();
            root
        };

        let tx = engine.begin_read();
        let tree = BTree::open(root);
        assert_eq!(tree.get(&tx, 1).unwrap().unwrap(), b"hello");
        assert_eq!(tree.get(&tx, 2).unwrap().unwrap(), b"world");
    }

    #[test]
    fn rollback_discards_changes() {
        let engine = StorageEngine::open_memory().unwrap();
        let tx = engine.begin_write();
        let root = BTree::create(&tx).unwrap();
        let tree = BTree::open(root);
        tree.insert(&tx, 1, b"x").unwrap();
        tx.commit().unwrap();

        // Start a write, insert, then roll back.
        let tx = engine.begin_write();
        let tree = BTree::open(root);
        tree.insert(&tx, 2, b"y").unwrap();
        tx.rollback();

        let tx = engine.begin_read();
        let tree = BTree::open(root);
        assert_eq!(tree.get(&tx, 1).unwrap().unwrap(), b"x");
        assert_eq!(tree.get(&tx, 2).unwrap(), None, "rolled back insert");
    }

    #[test]
    fn snapshot_isolation() {
        let engine = StorageEngine::open_memory().unwrap();
        // Seed one row.
        let tx = engine.begin_write();
        let root = BTree::create(&tx).unwrap();
        let tree = BTree::open(root);
        tree.insert(&tx, 1, b"v1").unwrap();
        tx.commit().unwrap();

        // Open a reader BEFORE the next commit.
        let reader = engine.begin_read();
        let tree = BTree::open(root);
        assert_eq!(tree.get(&reader, 1).unwrap().unwrap(), b"v1");

        // A writer commits a new row.
        let tx = engine.begin_write();
        tree.insert(&tx, 2, b"v2").unwrap();
        tx.commit().unwrap();

        // The pinned reader must NOT see the new row.
        assert_eq!(tree.get(&reader, 2).unwrap(), None, "snapshot isolation");

        // A fresh reader sees both rows.
        let reader2 = engine.begin_read();
        assert_eq!(tree.get(&reader2, 2).unwrap().unwrap(), b"v2");
    }
}
