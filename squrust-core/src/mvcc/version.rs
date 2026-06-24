//! Transaction identifiers and versioned pages.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::page::RawPage;

/// A monotonically increasing transaction / commit identifier.
pub type TransactionId = u64;

/// A global atomic counter used to hand out transaction ids.
#[derive(Debug, Default)]
pub struct VersionCounter(AtomicU64);

impl VersionCounter {
    pub fn new(start: u64) -> Self {
        VersionCounter(AtomicU64::new(start))
    }

    pub fn next(&self) -> TransactionId {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current(&self) -> TransactionId {
        self.0.load(Ordering::SeqCst)
    }

    pub fn set_at_least(&self, v: u64) {
        let mut cur = self.0.load(Ordering::SeqCst);
        while cur < v {
            match self
                .0
                .compare_exchange(cur, v, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
    }
}

/// A page snapshot tagged with the commit version that produced it.
#[derive(Debug, Clone)]
pub struct VersionedPage {
    pub version: TransactionId,
    pub page: Arc<RawPage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_increments() {
        let c = VersionCounter::new(0);
        assert_eq!(c.current(), 0);
        assert_eq!(c.next(), 1);
        assert_eq!(c.next(), 2);
        assert_eq!(c.current(), 2);
        c.set_at_least(10);
        assert_eq!(c.current(), 10);
        c.set_at_least(5); // no-op
        assert_eq!(c.current(), 10);
    }
}
