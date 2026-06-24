//! A simple, thread-safe LRU page cache holding `Arc<RawPage>` entries.
//!
//! Readers hold a clone of the `Arc`; the cache can evict its own reference
//! at any time and the page stays alive until the last reader drops it.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use super::{PageId, RawPage};

/// Default capacity: 2000 pages (~8 MiB at 4 KiB pages).
pub const DEFAULT_CAPACITY: usize = 2000;

struct Inner {
    capacity: usize,
    map: HashMap<PageId, Arc<RawPage>>,
    /// Most-recently-used at the back, least-recently-used at the front.
    lru: Vec<PageId>,
}

impl Inner {
    fn touch(&mut self, id: PageId) {
        if let Some(pos) = self.lru.iter().position(|&p| p == id) {
            self.lru.remove(pos);
        }
        self.lru.push(id);
    }

    fn evict_if_needed(&mut self) {
        while self.map.len() > self.capacity {
            if self.lru.is_empty() {
                break;
            }
            let victim = self.lru.remove(0);
            self.map.remove(&victim);
        }
    }
}

/// LRU cache of clean pages read from the main database file.
pub struct PageCache {
    inner: Mutex<Inner>,
}

impl PageCache {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        PageCache {
            inner: Mutex::new(Inner {
                capacity,
                map: HashMap::new(),
                lru: Vec::new(),
            }),
        }
    }

    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    pub fn get(&self, id: PageId) -> Option<Arc<RawPage>> {
        let mut inner = self.inner.lock();
        if let Some(page) = inner.map.get(&id).cloned() {
            inner.touch(id);
            Some(page)
        } else {
            None
        }
    }

    pub fn insert(&self, page: Arc<RawPage>) {
        let mut inner = self.inner.lock();
        let id = page.id;
        inner.map.insert(id, page);
        inner.touch(id);
        inner.evict_if_needed();
    }

    /// Drop a page from the cache (e.g. after it changed on disk).
    pub fn invalidate(&self, id: PageId) {
        let mut inner = self.inner.lock();
        inner.map.remove(&id);
        if let Some(pos) = inner.lru.iter().position(|&p| p == id) {
            inner.lru.remove(pos);
        }
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.map.clear();
        inner.lru.clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: PageId) -> Arc<RawPage> {
        Arc::new(RawPage::new(id))
    }

    #[test]
    fn evicts_least_recently_used() {
        let cache = PageCache::new(2);
        cache.insert(page(1));
        cache.insert(page(2));
        assert_eq!(cache.len(), 2);

        // Touch page 1 so page 2 becomes the LRU victim.
        assert!(cache.get(1).is_some());
        cache.insert(page(3));

        assert_eq!(cache.len(), 2);
        assert!(cache.get(1).is_some(), "1 was recently used");
        assert!(cache.get(2).is_none(), "2 should have been evicted");
        assert!(cache.get(3).is_some());
    }

    #[test]
    fn survives_via_arc_after_eviction() {
        let cache = PageCache::new(1);
        cache.insert(page(1));
        let held = cache.get(1).unwrap();
        cache.insert(page(2)); // evicts 1 from the cache
        assert!(cache.get(1).is_none());
        // The Arc the reader holds is still valid.
        assert_eq!(held.id, 1);
    }

    #[test]
    fn invalidate_removes() {
        let cache = PageCache::new(8);
        cache.insert(page(5));
        assert!(cache.get(5).is_some());
        cache.invalidate(5);
        assert!(cache.get(5).is_none());
    }
}
