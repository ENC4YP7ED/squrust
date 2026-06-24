//! Write-ahead log.
//!
//! Committed page versions are appended to a side file as [`WalFrame`]s and
//! also kept in an in-memory index so reads can find the right version for a
//! given snapshot without touching the main database file. This is what gives
//! us snapshot isolation: a reader pinned at version `V` only ever sees frames
//! whose commit version is `<= V`.

pub mod checkpoint;
pub mod frame;

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::error::Result;
use crate::page::{PAGE_SIZE, PageId};

pub use frame::{FRAME_SIZE, WalFrame};

type PageData = Arc<[u8; PAGE_SIZE]>;

struct WalInner {
    file: File,
    write_offset: u64,
    /// page id -> ascending list of (commit version, page data)
    index: HashMap<PageId, Vec<(u64, PageData)>>,
    max_version: u64,
    db_size: u32,
    salt: u32,
}

pub struct WriteAheadLog {
    inner: Mutex<WalInner>,
}

impl WriteAheadLog {
    /// Open the WAL beside the database file, replaying any committed frames.
    ///
    /// `initial_db_size` is the page count recorded in the main file header and
    /// is used when the WAL is empty.
    pub fn open(path: &Path, initial_db_size: u32) -> Result<WriteAheadLog> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        let mut inner = WalInner {
            file,
            write_offset: 0,
            index: HashMap::new(),
            max_version: 0,
            db_size: initial_db_size,
            salt: 0x5147_5253, // "SQRS"
        };
        Self::replay(&mut inner)?;

        Ok(WriteAheadLog {
            inner: Mutex::new(inner),
        })
    }

    /// Re-read all complete, committed transactions from the WAL file and
    /// rebuild the in-memory index. Any trailing partial transaction (a crash
    /// mid-write) is discarded and the file truncated back to the last good
    /// commit.
    fn replay(inner: &mut WalInner) -> Result<()> {
        let len = inner.file.metadata()?.len();
        let mut offset = 0u64;
        let mut last_committed_offset = 0u64;
        // Frames of the transaction currently being read.
        let mut pending: Vec<WalFrame> = Vec::new();

        while offset + FRAME_SIZE as u64 <= len {
            let mut buf = vec![0u8; FRAME_SIZE];
            if inner.file.read_exact_at(&mut buf, offset).is_err() {
                break;
            }
            let frame = match WalFrame::decode(&buf) {
                Ok(f) => f,
                Err(_) => break, // corrupt tail; stop here
            };
            let is_commit = frame.is_commit();
            let version = frame.commit_version;
            let db_size = frame.db_size_after;
            pending.push(frame);
            offset += FRAME_SIZE as u64;
            if is_commit {
                for f in pending.drain(..) {
                    inner
                        .index
                        .entry(f.page_id)
                        .or_default()
                        .push((version, Arc::new(*f.data)));
                }
                inner.max_version = inner.max_version.max(version);
                inner.db_size = db_size;
                last_committed_offset = offset;
            }
        }

        // Drop any partial trailing transaction.
        if last_committed_offset != len {
            inner.file.set_len(last_committed_offset)?;
        }
        inner.write_offset = last_committed_offset;
        Ok(())
    }

    pub fn max_version(&self) -> u64 {
        self.inner.lock().max_version
    }

    pub fn db_size(&self) -> u32 {
        self.inner.lock().db_size
    }

    pub fn salt(&self) -> u32 {
        self.inner.lock().salt
    }

    /// Append all pages of one committed transaction. The last frame must carry
    /// `db_size_after` so it is recognised as the commit marker. fsyncs before
    /// the in-memory index is updated, so a reader never observes a version
    /// that is not yet durable.
    pub fn append_commit(&self, mut frames: Vec<WalFrame>, version: u64, db_size: u32) -> Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        // Stamp version on every frame; mark only the last as the commit frame.
        let last = frames.len() - 1;
        for (i, f) in frames.iter_mut().enumerate() {
            f.commit_version = version;
            f.db_size_after = if i == last { db_size } else { 0 };
        }

        let mut inner = self.inner.lock();
        let mut offset = inner.write_offset;
        for f in &frames {
            let bytes = f.encode();
            inner.file.write_all_at(&bytes, offset)?;
            offset += FRAME_SIZE as u64;
        }
        inner.file.sync_all()?;
        inner.write_offset = offset;

        for f in frames {
            inner
                .index
                .entry(f.page_id)
                .or_default()
                .push((version, Arc::new(*f.data)));
        }
        inner.max_version = inner.max_version.max(version);
        inner.db_size = db_size;
        Ok(())
    }

    /// Latest version of `page_id` visible at snapshot `at_version`, if the WAL
    /// holds one.
    pub fn read_page(&self, page_id: PageId, at_version: u64) -> Option<PageData> {
        let inner = self.inner.lock();
        let versions = inner.index.get(&page_id)?;
        versions
            .iter()
            .rev()
            .find(|(v, _)| *v <= at_version)
            .map(|(_, d)| d.clone())
    }
}
