//! The 100-byte SQLite database header (offset 0 of page 1).
//!
//! Field layout per <https://www.sqlite.org/fileformat.html#the_database_header>.
//! Written so that a stock `sqlite3` can open files Squrust produces.

use crate::error::{Result, StorageError};
use crate::page::format::*;

#[derive(Debug, Clone, Copy)]
pub struct DbHeader {
    pub page_size: u32,
    pub db_size_pages: u32,
    pub change_counter: u32,
    pub schema_cookie: u32,
    pub freelist_trunk: u32,
    pub freelist_count: u32,
    /// The `PRAGMA user_version` value (header offset 60).
    pub user_version: u32,
}

impl Default for DbHeader {
    fn default() -> Self {
        DbHeader {
            page_size: PAGE_SIZE as u32,
            db_size_pages: 1,
            change_counter: 1,
            schema_cookie: 0,
            freelist_trunk: 0,
            freelist_count: 0,
            user_version: 0,
        }
    }
}

fn put_u32(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_be_bytes());
}
fn get_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

impl DbHeader {
    pub fn write_into(&self, buf: &mut [u8]) {
        assert!(buf.len() >= HEADER_SIZE);
        for b in buf[..HEADER_SIZE].iter_mut() {
            *b = 0;
        }
        buf[OFF_MAGIC..OFF_MAGIC + 16].copy_from_slice(MAGIC);

        let ps = if self.page_size == 65536 {
            1u16
        } else {
            self.page_size as u16
        };
        buf[OFF_PAGE_SIZE..OFF_PAGE_SIZE + 2].copy_from_slice(&ps.to_be_bytes());

        // Rollback-journal mode (1): a checkpointed file has no companion WAL,
        // so stock sqlite opens it directly.
        buf[OFF_WRITE_VERSION] = 1;
        buf[OFF_READ_VERSION] = 1;
        buf[OFF_RESERVED] = 0;
        buf[21] = 64; // max embedded payload fraction
        buf[22] = 32; // min embedded payload fraction
        buf[23] = 32; // leaf payload fraction

        put_u32(buf, OFF_FILE_CHANGE_COUNTER, self.change_counter);
        put_u32(buf, OFF_DB_SIZE_PAGES, self.db_size_pages);
        put_u32(buf, OFF_FREELIST_TRUNK, self.freelist_trunk);
        put_u32(buf, OFF_FREELIST_COUNT, self.freelist_count);
        put_u32(buf, OFF_SCHEMA_COOKIE, self.schema_cookie);
        put_u32(buf, OFF_SCHEMA_FORMAT, 4); // schema format number
        put_u32(buf, 48, 0); // default page cache size
        put_u32(buf, 52, 0); // largest root b-tree page (no auto-vacuum)
        put_u32(buf, OFF_TEXT_ENCODING, TEXT_ENCODING_UTF8);
        put_u32(buf, 60, self.user_version);
        put_u32(buf, 64, 0); // incremental-vacuum mode
        put_u32(buf, 68, 0); // application id
        // The in-header db size is only trusted when this equals the change
        // counter.
        put_u32(buf, OFF_VERSION_VALID_FOR, self.change_counter);
        put_u32(buf, OFF_SQLITE_VERSION, 3_045_000);
    }

    pub fn read_from(buf: &[u8]) -> Result<DbHeader> {
        if buf.len() < HEADER_SIZE || &buf[OFF_MAGIC..OFF_MAGIC + 16] != MAGIC {
            return Err(StorageError::BadMagic);
        }
        let raw_ps = u16::from_be_bytes([buf[OFF_PAGE_SIZE], buf[OFF_PAGE_SIZE + 1]]);
        let page_size = if raw_ps == 1 { 65536 } else { raw_ps as u32 };
        if page_size != PAGE_SIZE as u32 {
            return Err(StorageError::BadPageSize(page_size));
        }
        Ok(DbHeader {
            page_size,
            db_size_pages: get_u32(buf, OFF_DB_SIZE_PAGES),
            change_counter: get_u32(buf, OFF_FILE_CHANGE_COUNTER),
            schema_cookie: get_u32(buf, OFF_SCHEMA_COOKIE),
            freelist_trunk: get_u32(buf, OFF_FREELIST_TRUNK),
            freelist_count: get_u32(buf, OFF_FREELIST_COUNT),
            user_version: get_u32(buf, 60),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let mut buf = vec![0u8; PAGE_SIZE];
        let h = DbHeader {
            page_size: PAGE_SIZE as u32,
            db_size_pages: 42,
            change_counter: 7,
            schema_cookie: 3,
            freelist_trunk: 0,
            freelist_count: 0,
            user_version: 99,
        };
        h.write_into(&mut buf);
        assert_eq!(&buf[0..16], MAGIC);
        // db-size validity requires change counter == version-valid-for.
        assert_eq!(&buf[24..28], &buf[92..96]);
        let back = DbHeader::read_from(&buf).unwrap();
        assert_eq!(back.db_size_pages, 42);
        assert_eq!(back.change_counter, 7);
        assert_eq!(back.user_version, 99);
    }

    #[test]
    fn rejects_bad_magic() {
        let buf = vec![0u8; PAGE_SIZE];
        assert!(matches!(
            DbHeader::read_from(&buf),
            Err(StorageError::BadMagic)
        ));
    }
}
