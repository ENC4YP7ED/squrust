//! Multi-version concurrency control.
//!
//! Concurrency model: a single writer at a time (serialised by the storage
//! engine) plus any number of readers. Each commit is stamped with an
//! increasing version. Readers capture the latest committed version when they
//! begin and only ever observe pages at or below that version, which the WAL
//! index serves. This is the same snapshot-isolation discipline SQLite uses in
//! WAL mode.

pub mod snapshot;
pub mod version;

use std::collections::BTreeMap;

use parking_lot::Mutex;

pub use snapshot::Snapshot;
pub use version::{TransactionId, VersionCounter};

pub struct MvccManager {
    /// The latest durably-committed version.
    committed: VersionCounter,
    /// Active read snapshots: version -> reference count. Used to find the
    /// oldest version still required by a live reader (so a checkpoint knows
    /// what it may discard).
    active_readers: Mutex<BTreeMap<TransactionId, usize>>,
    /// Hands out unique write-transaction ids.
    tx_ids: VersionCounter,
}

impl MvccManager {
    pub fn new(initial_committed: TransactionId) -> Self {
        MvccManager {
            committed: VersionCounter::new(initial_committed),
            active_readers: Mutex::new(BTreeMap::new()),
            tx_ids: VersionCounter::new(initial_committed),
        }
    }

    pub fn current_version(&self) -> TransactionId {
        self.committed.current()
    }

    /// Begin a read transaction, pinning the current committed version.
    pub fn begin_read(&self) -> Snapshot {
        let v = self.committed.current();
        *self.active_readers.lock().entry(v).or_insert(0) += 1;
        Snapshot::new(v)
    }

    /// Release a previously acquired read snapshot.
    pub fn end_read(&self, snapshot: Snapshot) {
        let mut readers = self.active_readers.lock();
        if let Some(count) = readers.get_mut(&snapshot.version) {
            *count -= 1;
            if *count == 0 {
                readers.remove(&snapshot.version);
            }
        }
    }

    /// Allocate a fresh write-transaction id.
    pub fn begin_write(&self) -> TransactionId {
        self.tx_ids.next()
    }

    /// The version a new commit will be stamped with (committed + 1). Not yet
    /// published; call [`publish_commit`](Self::publish_commit) after the WAL
    /// write succeeds.
    pub fn reserve_commit_version(&self) -> TransactionId {
        self.committed.current() + 1
    }

    /// Make a freshly-committed version visible to new readers.
    pub fn publish_commit(&self, version: TransactionId) {
        self.committed.set_at_least(version);
        self.tx_ids.set_at_least(version);
    }

    /// The oldest version any live reader still depends on, or the current
    /// committed version if there are no readers.
    pub fn oldest_required_version(&self) -> TransactionId {
        let readers = self.active_readers.lock();
        readers
            .keys()
            .next()
            .copied()
            .unwrap_or_else(|| self.committed.current())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_pins_version() {
        let m = MvccManager::new(5);
        let r1 = m.begin_read();
        assert_eq!(r1.version, 5);

        // A writer commits version 6.
        let v = m.reserve_commit_version();
        assert_eq!(v, 6);
        m.publish_commit(v);

        // Existing reader still pinned at 5; a new reader sees 6.
        assert_eq!(r1.version, 5);
        assert!(!r1.sees(6));
        let r2 = m.begin_read();
        assert_eq!(r2.version, 6);
        assert!(r2.sees(6));

        assert_eq!(m.oldest_required_version(), 5);
        m.end_read(r1);
        assert_eq!(m.oldest_required_version(), 6);
        m.end_read(r2);
    }
}
