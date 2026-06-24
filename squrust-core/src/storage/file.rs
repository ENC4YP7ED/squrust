//! The main database file.
//!
//! Uses positioned reads/writes (`pread`/`pwrite` via `FileExt`) rather than
//! `mmap`, which keeps the whole crate free of `unsafe` while still giving
//! random-access page I/O. Page 1 carries the 100-byte header in its first
//! bytes.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::Path;

use parking_lot::Mutex;

use crate::error::Result;
use crate::page::{PAGE_SIZE, PageId};

use super::header::DbHeader;

/// A handle to the on-disk database file.
pub struct DatabaseFile {
    file: Mutex<File>,
}

impl DatabaseFile {
    /// Open (creating if missing) the database file at `path`.
    ///
    /// Returns the file handle and the parsed header. A freshly created file
    /// is initialised with a default header on page 1.
    pub fn open(path: &Path) -> Result<(DatabaseFile, DbHeader)> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let len = file.metadata()?.len();
        let db = DatabaseFile {
            file: Mutex::new(file),
        };
        let header = if len == 0 {
            let header = DbHeader::default();
            db.init(&header)?;
            header
        } else {
            let mut first = vec![0u8; PAGE_SIZE];
            db.file.lock().read_exact_at(&mut first, 0)?;
            DbHeader::read_from(&first)?
        };
        Ok((db, header))
    }

    /// Write the initial page 1: the 100-byte header followed by an empty
    /// `sqlite_master` table-leaf b-tree (so a fresh file is a valid, empty
    /// SQLite database).
    fn init(&self, header: &DbHeader) -> Result<()> {
        use crate::btree::node::Node;
        let mut page = crate::page::RawPage::new(1);
        header.write_into(&mut page.data[..]);
        Node::empty_leaf().serialize_into(&mut page);
        let file = self.file.lock();
        file.write_all_at(&page.data[..], 0)?;
        file.sync_all()?;
        Ok(())
    }

    /// Number of full pages currently in the file.
    pub fn page_count(&self) -> Result<u32> {
        let len = self.file.lock().metadata()?.len();
        Ok((len / PAGE_SIZE as u64) as u32)
    }

    fn offset(id: PageId) -> u64 {
        // Page ids are 1-based.
        (id as u64 - 1) * PAGE_SIZE as u64
    }

    /// Read page `id`. Returns `None` if the page is past end-of-file.
    pub fn read_page(&self, id: PageId) -> Result<Option<Box<[u8; PAGE_SIZE]>>> {
        if id == 0 {
            return Ok(None);
        }
        let file = self.file.lock();
        let len = file.metadata()?.len();
        let off = Self::offset(id);
        if off + PAGE_SIZE as u64 > len {
            return Ok(None);
        }
        let mut buf = Box::new([0u8; PAGE_SIZE]);
        file.read_exact_at(buf.as_mut_slice(), off)?;
        Ok(Some(buf))
    }

    /// Write page `id`, growing the file if needed.
    pub fn write_page(&self, id: PageId, data: &[u8]) -> Result<()> {
        debug_assert!(data.len() == PAGE_SIZE);
        let file = self.file.lock();
        file.write_all_at(data, Self::offset(id))?;
        Ok(())
    }

    /// Overwrite just the header region of page 1.
    pub fn write_header(&self, header: &DbHeader) -> Result<()> {
        // Read page 1, patch the header bytes, write it back so we don't clobber
        // any b-tree content that shares page 1.
        let mut page = match self.read_page(1)? {
            Some(p) => p,
            None => Box::new([0u8; PAGE_SIZE]),
        };
        header.write_into(&mut page[..]);
        self.write_page(1, &page[..])
    }

    pub fn sync(&self) -> Result<()> {
        self.file.lock().sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let (db, header) = DatabaseFile::open(&path).unwrap();
            assert_eq!(header.page_size, PAGE_SIZE as u32);
            assert_eq!(db.page_count().unwrap(), 1);
        }
        // Reopen: header must still parse.
        let (_db, header) = DatabaseFile::open(&path).unwrap();
        assert_eq!(header.page_size, PAGE_SIZE as u32);
    }

    #[test]
    fn page_write_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let (db, _h) = DatabaseFile::open(&path).unwrap();
        let mut data = [0u8; PAGE_SIZE];
        data[0] = 0xAA;
        data[PAGE_SIZE - 1] = 0xBB;
        db.write_page(5, &data).unwrap();
        let back = db.read_page(5).unwrap().unwrap();
        assert_eq!(back[0], 0xAA);
        assert_eq!(back[PAGE_SIZE - 1], 0xBB);
        // Page 3 was never written; reads as zeros (file grew to cover it).
        assert!(db.read_page(99).unwrap().is_none());
    }
}
