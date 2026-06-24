//! A read snapshot: the database version a read transaction observes.

use super::version::TransactionId;

/// A point-in-time view of the database. A reader created at version `V` sees
/// every commit with version `<= V` and nothing newer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub version: TransactionId,
}

impl Snapshot {
    pub fn new(version: TransactionId) -> Self {
        Snapshot { version }
    }

    /// Whether a page committed at `committed_version` is visible to this
    /// snapshot.
    pub fn sees(&self, committed_version: TransactionId) -> bool {
        committed_version <= self.version
    }
}
