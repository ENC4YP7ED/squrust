//! Checkpointing: fold committed WAL frames back into the main database file
//! and reset the log.

use crate::error::Result;
use crate::page::cache::PageCache;
use crate::storage::file::DatabaseFile;
use crate::storage::header::DbHeader;

use super::WriteAheadLog;

impl WriteAheadLog {
    /// Write the most recent committed version of every page in the WAL into
    /// the main file, fsync, then truncate the WAL. The page cache is
    /// invalidated for every checkpointed page so subsequent reads see the
    /// fresh on-disk data.
    ///
    /// Note: this is an "all" checkpoint; callers must ensure no reader still
    /// requires an older snapshot.
    pub fn checkpoint(&self, file: &DatabaseFile, cache: &PageCache) -> Result<()> {
        let mut inner = self.inner.lock();
        if inner.index.is_empty() {
            return Ok(());
        }

        // Highest version of each page wins.
        let mut pages: Vec<(u32, std::sync::Arc<[u8; crate::page::PAGE_SIZE]>)> = Vec::new();
        for (&page_id, versions) in inner.index.iter() {
            if let Some((_, data)) = versions.last() {
                pages.push((page_id, data.clone()));
            }
        }
        pages.sort_by_key(|(id, _)| *id);

        for (page_id, data) in &pages {
            file.write_page(*page_id, &data[..])?;
            cache.invalidate(*page_id);
        }

        // Persist a valid SQLite header. `db_size_pages` must cover every page
        // physically present so stock sqlite reads the whole file; the change
        // counter doubles as version-valid-for (set inside `write_into`).
        let db_size = file.page_count()?.max(inner.db_size);
        // Preserve `PRAGMA user_version`: read it back from page 1 (which now
        // reflects any committed change) before rewriting the header region.
        let user_version = file
            .read_page(1)?
            .map(|p| u32::from_be_bytes([p[60], p[61], p[62], p[63]]))
            .unwrap_or(0);
        let header = DbHeader {
            page_size: crate::page::PAGE_SIZE as u32,
            db_size_pages: db_size,
            change_counter: (inner.max_version as u32).max(1),
            schema_cookie: 1,
            freelist_trunk: 0,
            freelist_count: 0,
            user_version,
        };
        file.write_header(&header)?;
        file.sync()?;

        // Reset the log: truncate the file and clear the in-memory index.
        inner.file.set_len(0)?;
        inner.write_offset = 0;
        inner.index.clear();
        // max_version is retained so future commits keep increasing.
        Ok(())
    }
}
