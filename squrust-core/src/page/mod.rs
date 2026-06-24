//! Pages: the fixed-size unit of storage.

pub mod cache;
pub mod format;

pub use format::{HEADER_SIZE, PAGE_SIZE};

/// A page identifier. Page 1 is the header/root page (SQLite convention).
pub type PageId = u32;

/// The logical type of a page, encoded in the first byte of the page body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageType {
    Interior,
    Leaf,
    Overflow,
    FreeList,
}

impl PageType {
    pub fn to_byte(self) -> u8 {
        match self {
            PageType::Interior => 0x05,
            PageType::Leaf => 0x0d,
            PageType::Overflow => 0x02,
            PageType::FreeList => 0x00,
        }
    }

    pub fn from_byte(b: u8) -> Option<PageType> {
        match b {
            0x05 => Some(PageType::Interior),
            0x0d => Some(PageType::Leaf),
            0x02 => Some(PageType::Overflow),
            0x00 => Some(PageType::FreeList),
            _ => None,
        }
    }
}

/// A raw page: a page id plus exactly `PAGE_SIZE` bytes.
#[derive(Clone)]
pub struct RawPage {
    pub id: PageId,
    pub data: Box<[u8; PAGE_SIZE]>,
    pub dirty: bool,
}

impl RawPage {
    /// A fresh zeroed page.
    pub fn new(id: PageId) -> Self {
        RawPage {
            id,
            data: Box::new([0u8; PAGE_SIZE]),
            dirty: false,
        }
    }

    /// Build a page from raw bytes (must be exactly `PAGE_SIZE`).
    pub fn from_bytes(id: PageId, bytes: &[u8]) -> Self {
        let mut data = Box::new([0u8; PAGE_SIZE]);
        let n = bytes.len().min(PAGE_SIZE);
        data[..n].copy_from_slice(&bytes[..n]);
        RawPage {
            id,
            data,
            dirty: false,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data[..]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.dirty = true;
        &mut self.data[..]
    }
}

impl std::fmt::Debug for RawPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawPage")
            .field("id", &self.id)
            .field("dirty", &self.dirty)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_roundtrip() {
        let mut p = RawPage::new(7);
        assert_eq!(p.id, 7);
        assert!(!p.dirty);
        p.as_mut_slice()[10] = 0xAB;
        assert!(p.dirty);
        let bytes = p.as_slice().to_vec();
        assert_eq!(bytes.len(), PAGE_SIZE);
        let q = RawPage::from_bytes(7, &bytes);
        assert_eq!(q.data[10], 0xAB);
        assert!(!q.dirty);
    }

    #[test]
    fn page_type_roundtrip() {
        for t in [
            PageType::Interior,
            PageType::Leaf,
            PageType::Overflow,
            PageType::FreeList,
        ] {
            assert_eq!(PageType::from_byte(t.to_byte()), Some(t));
        }
        assert_eq!(PageType::from_byte(0xFF), None);
    }
}
